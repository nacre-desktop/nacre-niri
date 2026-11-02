#!/usr/bin/env python3
"""Exercise nacre-niri-session with private HOME and mocked user services."""

import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
WRAPPER = ROOT / "nacre-niri-session"


def run(active="", start_fails=False, with_nacre=False, no_config=False):
    with tempfile.TemporaryDirectory(prefix="nacre-niri-session-") as temp:
        root = Path(temp)
        home = root / "home"
        bindir = root / "bin"
        home.mkdir()
        bindir.mkdir()
        log = root / "calls"
        config = home / (
            ".local/share/nacre/niri-on-pc/session/config/config.kdl"
            if with_nacre
            else ".config/nacre/niri/config.kdl"
        )
        if not no_config:
            config.parent.mkdir(parents=True)
            config.write_text("# private test config\n")
            if with_nacre:
                (home / ".local/share/nacre/niri-on-pc/session/source-revision").write_text("test\n")
        systemctl = bindir / "systemctl"
        systemctl.write_text(
            "#!/bin/sh\n"
            "printf 'systemctl|%s|NIRI_CONFIG=%s\\n' \"$*\" \"${NIRI_CONFIG-}\" >> \"$TEST_LOG\"\n"
            "if [ \"$3\" = is-active ]; then\n"
            "  case \",${ACTIVE_UNITS-},\" in *\",$4,\"*) exit 0;; esac\n"
            "  exit 3\n"
            "fi\n"
            "if [ \"$*\" = '--user --wait start nacre-niri.service' ] && [ \"${START_FAIL-0}\" = 1 ]; then exit 31; fi\n"
        )
        systemctl.chmod(0o755)
        dbus = bindir / "dbus-update-activation-environment"
        dbus.write_text(
            "#!/bin/sh\nprintf 'dbus|%s|NIRI_CONFIG=%s\\n' \"$*\" \"${NIRI_CONFIG-}\" >> \"$TEST_LOG\"\n"
        )
        dbus.chmod(0o755)
        if with_nacre:
            uwsm = bindir / "uwsm"
            uwsm.write_text(
                "#!/bin/sh\nprintf 'uwsm|%s|NIRI_CONFIG=%s|NACRE_COMPOSITOR=%s|WAIT=%s|FINALIZE=%s\\n' \"$*\" \"${NIRI_CONFIG-}\" \"${NACRE_COMPOSITOR-}\" \"${UWSM_WAIT_VARNAMES-}\" \"${UWSM_FINALIZE_VARNAMES-}\" >> \"$TEST_LOG\"\n"
            )
            uwsm.chmod(0o755)
            env_niri_config = str(config)
        else:
            env_niri_config = str(config)
        env = {
            **os.environ,
            "HOME": str(home),
            "PATH": str(bindir),
            "TEST_LOG": str(log),
            "ACTIVE_UNITS": active,
            "START_FAIL": "1" if start_fails else "0",
            "NIRI_CONFIG": env_niri_config,
        }
        env.pop("MANAGERPID", None)
        env.pop("SYSTEMD_EXEC_PID", None)
        result = subprocess.run([str(WRAPPER)], env=env, check=False)
        calls = log.read_text().splitlines() if log.exists() else []
        return result.returncode, calls, env_niri_config


def check(condition, message):
    if not condition:
        raise AssertionError(message)


for unit in ("graphical-session.target", "niri.service"):
    code, calls, _ = run(active=unit)
    check(code != 0, f"accepted active {unit}")
    check(not any("reset-failed" in call or "import-environment" in call for call in calls),
          f"mutated systemd before refusing active {unit}: {calls}")

code, calls, _ = run(no_config=True)
check(code != 0 and not any("reset-failed" in call or "import-environment" in call for call in calls),
      f"session without a Nacre config was not refused safely: {code}, {calls}")

code, calls, configured = run()
check(code == 0, f"normal start failed: {code}, {calls}")
check(any("reset-failed nacre-niri.service nacre-niri-shutdown.target" in call for call in calls),
      f"did not reset only renamed units: {calls}")
check(any("--wait start nacre-niri.service" in call for call in calls),
      f"did not start renamed service: {calls}")
check(any("nacre-niri-shutdown.target" in call for call in calls),
      f"did not stop through renamed target: {calls}")
check(any("unset-environment" in call and "NACRE_NIRI_FORK" in call and "NIRI_CONFIG" in call for call in calls),
      f"did not clean fork session variables from systemd: {calls}")
check(all(f"NIRI_CONFIG={configured}" in call
          for call in calls if "import-environment" in call or call.startswith("dbus|")),
      f"lost shared Nacre config in imported environment: {calls}")

code, calls, _ = run(start_fails=True)
check(code == 31, f"service failure status was hidden: {code}, {calls}")
check(any("start --job-mode=replace-irreversibly nacre-niri-shutdown.target" in call for call in calls),
      f"did not clean up failed compositor session: {calls}")
code, calls, config = run(with_nacre=True)
uwsm_call = next((call for call in calls if call.startswith("uwsm|")), "")
check(code == 0 and f"uwsm|start -e -D niri -- /usr/bin/nacre-niri --session --config {config}" in uwsm_call,
      f"shared-config UWSM launch failed: {code}, {calls}")
check(f"NIRI_CONFIG={config}" in uwsm_call and "NACRE_COMPOSITOR=niri" in uwsm_call,
      f"lost Nacre session environment: {calls}")
check("NIRI_SOCKET" in uwsm_call and "NACRE_NIRI_FORK" in uwsm_call,
      f"UWSM did not manage Niri session variables: {calls}")
print("PASS: active-session guards, renamed unit routing, shared NIRI_CONFIG, error cleanup, UWSM config launch")
