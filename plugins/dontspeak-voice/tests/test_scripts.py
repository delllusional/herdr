import importlib.util
import os
from pathlib import Path
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]


def load_script(name):
    path = ROOT / "scripts" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


dictation = load_script("dictation")
status_bridge = load_script("status_bridge")


class StatusBridgeTests(unittest.TestCase):
    def test_popup_is_scoped_to_the_observed_dictation_session(self):
        with (
            mock.patch.object(status_bridge, "herdr_foreground", return_value=True),
            mock.patch.object(
                status_bridge, "command", return_value=mock.Mock(returncode=0)
            ) as command,
        ):
            self.assertTrue(status_bridge.open_dictation_popup("session-7"))

        argv = command.call_args.args
        self.assertIn("--no-focus", argv)
        self.assertIn("--env", argv)
        self.assertIn("DONTSPEAK_DICTATION_SESSION=session-7", argv)

    def test_background_herdr_never_opens_popup(self):
        with (
            mock.patch.object(status_bridge, "herdr_foreground", return_value=False),
            mock.patch.object(status_bridge, "command") as command,
        ):
            self.assertFalse(status_bridge.open_dictation_popup("session-7"))
        command.assert_not_called()

    def test_foreground_signal_is_fail_closed(self):
        snapshots = (
            mock.Mock(
                returncode=0,
                stdout='{"result":{"snapshot":{"outer_terminal_focus":true}}}',
            ),
            mock.Mock(
                returncode=0,
                stdout='{"result":{"snapshot":{"outer_terminal_focus":false}}}',
            ),
            mock.Mock(returncode=0, stdout='{"result":{"snapshot":{}}}'),
            mock.Mock(returncode=0, stdout="not-json"),
            mock.Mock(returncode=1, stdout=""),
        )
        expected = (True, False, False, False, False)
        for response, is_foreground in zip(snapshots, expected):
            with self.subTest(response=response.stdout):
                with mock.patch.object(
                    status_bridge, "command", return_value=response
                ):
                    self.assertEqual(
                        status_bridge.herdr_foreground(),
                        is_foreground,
                    )

    def test_manual_action_opens_only_the_current_visible_session(self):
        response = mock.Mock(
            returncode=0,
            stdout='{"dictation":{"state":"recording","session_id":"session-7"}}',
        )
        with (
            mock.patch.object(status_bridge, "command", return_value=response),
            mock.patch.object(
                status_bridge, "open_dictation_popup", return_value=True
            ) as open_popup,
        ):
            self.assertTrue(status_bridge.open_current_dictation())
        open_popup.assert_called_once_with("session-7")

        for state in ("hidden", "refused"):
            response.stdout = (
                f'{{"dictation":{{"state":"{state}","session_id":"session-8"}}}}'
            )
            with (
                mock.patch.object(status_bridge, "command", return_value=response),
                mock.patch.object(status_bridge, "open_dictation_popup") as open_popup,
            ):
                self.assertFalse(status_bridge.open_current_dictation())
            open_popup.assert_not_called()

    def test_voice_token_priority_is_stable(self):
        row = {"voice": "Sarah", "speaking": True, "queued": 2, "blocked": True}
        self.assertEqual(status_bridge.token_for(row, False), "SPEAK Sarah")
        self.assertEqual(status_bridge.token_for(row, True), "MUTE Sarah")

    def test_popup_retry_interval_exceeds_the_presenter_lease(self):
        self.assertGreater(
            status_bridge.POPUP_RETRY_SECS * 1000,
            dictation.LEASE_TTL_MS,
        )


class DictationPresenterTests(unittest.TestCase):
    def test_popup_owns_the_lease_only_after_first_render(self):
        snapshots = [
            {
                "seq": 1,
                "dictation": {
                    "state": "recording",
                    "session_id": "session-7",
                    "text": "hel",
                },
            },
            {
                "seq": 2,
                "dictation": {
                    "state": "recording",
                    "session_id": "session-7",
                    "text": "hello",
                },
            },
            {
                "seq": 3,
                "dictation": {
                    "state": "hidden",
                    "session_id": None,
                    "text": "",
                },
            },
        ]
        calls = []

        def presenter(action, session_id, lease_id=None):
            calls.append((action, session_id, lease_id))
            return "lease-1" if action == "acquire" else True

        with (
            mock.patch.dict(
                os.environ,
                {"DONTSPEAK_DICTATION_SESSION": "session-7"},
                clear=False,
            ),
            mock.patch.object(dictation, "status", side_effect=snapshots),
            mock.patch.object(dictation, "presenter", side_effect=presenter),
            mock.patch.object(dictation, "herdr_foreground", return_value=True),
            mock.patch.object(dictation, "draw") as draw,
        ):
            dictation.main()

        self.assertEqual(
            calls,
            [
                ("acquire", "session-7", None),
                ("ready", "session-7", "lease-1"),
                ("renew", "session-7", "lease-1"),
                ("release", "session-7", "lease-1"),
            ],
        )
        self.assertEqual([call.args[0] for call in draw.call_args_list], ["hel", "hello"])

    def test_session_race_never_acquires_or_hides_native_fallback(self):
        snapshot = {
            "seq": 1,
            "dictation": {
                "state": "recording",
                "session_id": "new-session",
                "text": "new",
            },
        }
        with (
            mock.patch.dict(
                os.environ,
                {"DONTSPEAK_DICTATION_SESSION": "old-session"},
                clear=False,
            ),
            mock.patch.object(dictation, "status", return_value=snapshot),
            mock.patch.object(dictation, "presenter") as presenter,
        ):
            dictation.main()
        presenter.assert_not_called()

    def test_refusal_cue_stays_with_the_native_presenter(self):
        snapshot = {
            "seq": 1,
            "dictation": {
                "state": "refused",
                "session_id": "session-7",
                "text": "",
            },
        }
        with (
            mock.patch.dict(
                os.environ,
                {"DONTSPEAK_DICTATION_SESSION": "session-7"},
                clear=False,
            ),
            mock.patch.object(dictation, "status", return_value=snapshot),
            mock.patch.object(dictation, "presenter") as presenter,
        ):
            dictation.main()
        presenter.assert_not_called()

    def test_popup_exits_when_status_is_lost_for_a_full_lease(self):
        snapshot = {
            "seq": 1,
            "dictation": {
                "state": "recording",
                "session_id": "session-7",
                "text": "hello",
            },
        }
        calls = []

        def presenter(action, session_id, lease_id=None):
            calls.append(action)
            return "lease-1" if action == "acquire" else True

        with (
            mock.patch.dict(
                os.environ,
                {"DONTSPEAK_DICTATION_SESSION": "session-7"},
                clear=False,
            ),
            mock.patch.object(dictation, "status", side_effect=[snapshot, None]),
            mock.patch.object(dictation, "presenter", side_effect=presenter),
            mock.patch.object(dictation, "herdr_foreground", return_value=True),
            mock.patch.object(dictation, "draw"),
            mock.patch.object(dictation.time, "monotonic", side_effect=[0, 4]),
        ):
            dictation.main()

        self.assertEqual(calls, ["acquire", "ready", "release"])

    def test_focus_loss_releases_ready_lease_without_renewing(self):
        snapshots = [
            {
                "seq": 1,
                "dictation": {
                    "state": "recording",
                    "session_id": "session-7",
                    "text": "hel",
                },
            },
            {
                "seq": 2,
                "dictation": {
                    "state": "recording",
                    "session_id": "session-7",
                    "text": "hello",
                },
            },
        ]
        calls = []

        def presenter(action, session_id, lease_id=None):
            calls.append(action)
            return "lease-1" if action == "acquire" else True

        with (
            mock.patch.dict(
                os.environ,
                {"DONTSPEAK_DICTATION_SESSION": "session-7"},
                clear=False,
            ),
            mock.patch.object(dictation, "status", side_effect=snapshots),
            mock.patch.object(dictation, "presenter", side_effect=presenter),
            mock.patch.object(
                dictation,
                "herdr_foreground",
                side_effect=[True, True, False],
            ),
            mock.patch.object(dictation, "draw"),
        ):
            dictation.main()

        self.assertEqual(calls, ["acquire", "ready", "release"])


if __name__ == "__main__":
    unittest.main()
