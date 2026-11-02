# NN0003 packaging evidence

- Source compatibility commit: `f1dea218011ac2fa6129ebb03187570ea59d254c`; author and committer are `siverteh <92440237+Siverteh@users.noreply.github.com>`.
- Packaging commit: `a386b1d9d17824001e0da0dfecc74779551ca63e`; task branch `task/codex-nn0003-package-20261102` points to it.
- Package build: `docker exec nn0003-package-build-20261102 su - builder -c "cd /src/packaging/arch && nice -n 10 makepkg --nodeps --noconfirm"`; completed as non-root, `nacre-niri 26.04.r2795.gf1dea218-1`. Package was not installed.
- Artifact: `nacre-niri-26.04.r2795.gf1dea218-1-x86_64.pkg.tar.zst`; SHA256 `ef10ed8b6c552098ddd9149084ee5c130b62a65979c616e940dffabf46e86737`.
- Laptop copy: `/home/siverteh/.local/share/nacre/ai/lead/packages/nacre-niri-26.04.r2795.gf1dea218-1-x86_64.pkg.tar.zst`; local SHA256 matched the remote build artifact.
- Non-directory payload: `/usr/bin/nacre-niri`, `/usr/bin/nacre-niri-session`, `/usr/share/wayland-sessions/nacre-niri.desktop`, `/usr/lib/systemd/user/nacre-niri.service`, `/usr/lib/systemd/user/nacre-niri-shutdown.target`.
- Version output: `niri 26.04 (nacre-niri f1dea218011ac2fa6129ebb03187570ea59d254c)`.
- File conflict audit: compared package payload files with `pacman -Ql niri` on installed stock `niri 26.04-1`; overlap output was empty. `.PKGINFO` contains no `provides`, `conflicts`, or `replaces` fields.
- `namcap PKGBUILD nacre-niri-26.04.r2795.gf1dea218-1-x86_64.pkg.tar.zst`: exit 0, five warnings and no errors. Raw output:

  ```text
  nacre-niri W: Referenced library 'bash' is an uninstalled dependency (needed in files ['usr/bin/nacre-niri-session'])
  nacre-niri W: Unused shared library '/usr/lib64/ld-linux-x86-64.so.2' by file ('usr/bin/nacre-niri')
  nacre-niri W: Dependency included, but may not be needed ('niri')
  nacre-niri W: Dependency included, but may not be needed ('systemd')
  nacre-niri W: Dependency included, but may not be needed ('bash')
  ```

  Bash warnings reflect the `/usr/bin/bash` wrapper interpreter; `pacman -Q bash` and `pacman -Qo /usr/bin/bash` confirm Bash 5.3.20-1 is installed and owns it. `niri` and `systemd` are intentional dependencies retaining the stock rollback compositor and providing the session manager. The loader warning is a false positive for the ELF interpreter: `readelf -l` reports `/lib64/ld-linux-x86-64.so.2`, `ldd` resolves it, and `pacman -Qo` maps the loader to glibc. `ldd` reported no missing libraries.
- Private-HOME session test: `nice -n 10 python test-session-wrapper.py` printed `PASS: active-session guards, renamed unit routing, shared NIRI_CONFIG, error cleanup, UWSM config launch`.
- IPC fixture: `nice -n 10 python test-ipc.py /tmp/nn0003-extract/usr/bin/nacre-niri /usr/bin/niri` printed `PASS: fork identity, query IPC, stock-compatible default event streams, collections opt-in`. The clients used a private Unix socket fixture; this did not run a compositor.
- Final targeted check: `nice -n 10 rustfmt --check --edition 2021 src/cli.rs src/ipc/client.rs src/ipc/server.rs niri-ipc/src/lib.rs && nice -n 10 cargo test --release -j2 --lib ipc::server::tests`; rustfmt passed with stable-toolchain warnings about nightly-only config settings. Test output: `running 2 tests`, both server snapshot/broadcast tests passed, `2 passed; 0 failed; 215 filtered out`; rustc also showed one unrelated pre-existing unused `MergeWith` import warning in `src/layout/tests.rs`.
- IPC fixture was hardened after review to bind before starting its server thread, set timeouts, and derive the expected revision from `--version`; rerun passed against the final archive.
- Workspace-wide `cargo fmt --all -- --check` reports formatting differences in untouched `src/protocols/foreign_toplevel.rs`; the changed files passed targeted `rustfmt --check --edition 2021`.
- The desktop TEST host was unavailable for a nested winit session and overview screenshot; no real compositor was run.
