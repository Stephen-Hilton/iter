#!/usr/bin/env bash
# iter_engine_setup.sh - retired 2026-10-11. ONE engine per machine is now set
# up with the install script in the iter repository, which makes the machine's
# own engine key (nothing secret is copied anywhere):
#
#   Linux:   cd ~/dev && git clone https://github.com/Stephen-Hilton/iter.git && ./iter/install.sh
#   Windows: cd ~\dev; git clone https://github.com/Stephen-Hilton/iter.git; .\iter\install.ps1
#
# iter_data still serves this file at /iter_engine_setup.sh so an old link
# says where to go instead of failing.
cat >&2 <<'MSG'
iter_engine_setup.sh is retired. Set up this machine's engine (ONE engine per machine) with:

  Linux:   cd ~/dev && git clone https://github.com/Stephen-Hilton/iter.git && ./iter/install.sh
  Windows: cd ~\dev; git clone https://github.com/Stephen-Hilton/iter.git; .\iter\install.ps1
MSG
exit 1
