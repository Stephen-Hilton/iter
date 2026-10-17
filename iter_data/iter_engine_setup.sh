#!/usr/bin/env bash
# iter_engine_setup.sh - retired 2026-10-11. ONE engine per machine is now set
# up with the install script in the iter repository, which makes the machine's
# own engine key (nothing secret is copied anywhere):
#
#   installers/iter_engine/<linux|linux_wsl|macos|docker|windows>/install.sh (.ps1),
#   run straight from raw.githubusercontent.com; each downloads the prebuilt
#   iter_engine from the GitHub Release (the message below has the lines)
#
# iter_data still serves this file at /iter_engine_setup.sh so an old link
# says where to go instead of failing.
cat >&2 <<'MSG'
iter_engine_setup.sh is retired. Set up this machine's engine (ONE engine per machine) with:

  Linux:    curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux/install.sh | bash
  WSL2:     curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/linux_wsl/install.sh | bash
  macOS:    curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/macos/install.sh | bash
  Docker:   curl -fsSL https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/docker/install.sh | bash
  Windows:  irm https://raw.githubusercontent.com/Stephen-Hilton/iter/main/installers/iter_engine/windows/install.ps1 -OutFile $env:TEMP\iter-install.ps1; powershell -ExecutionPolicy Bypass -File $env:TEMP\iter-install.ps1

Each one downloads a prebuilt iter_engine from https://github.com/Stephen-Hilton/iter/releases
(add --version vYYYY.MM.DD, Windows: -Version, for a given release); see installers/iter_engine/ in the repo.
MSG
exit 1
