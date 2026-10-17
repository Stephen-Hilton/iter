#!/usr/bin/env bash
# installers/build.sh - build every iter release binary, put one copy of each
# into installers/dist/ (plus SHA256SUMS), and copy into each installer folder
# the assets that folder can use. With --release, also tag HEAD and publish
# dist/* as a GitHub Release.
#
#   installers/build.sh                          build all 7 assets + place them
#   installers/build.sh --only 'iter_engine-macos-*'
#   installers/build.sh --skip-windows
#   installers/build.sh --dry-run                print every action, change nothing
#   installers/build.sh --release                build all + tag + gh release create
#
# Written for an Apple-silicon Mac with Docker running:
#   macOS aarch64   native cargo
#   macOS x86_64    cargo --target x86_64-apple-darwin
#   Linux aarch64   inside a linux/arm64 rust:1-bullseye container (glibc 2.31)
#   Linux x86_64    cross-compiled in that same container (gcc-x86-64-linux-gnu)
#   Windows x86_64  cargo xwin (x86_64-pc-windows-msvc, static CRT; Microsoft
#                   CRT + SDK downloaded by cargo-xwin; needs `brew install llvm`)
# Container builds use the docker volumes iter-build-cargo (registry/git
# cache) and iter-build-target (target dir), never the host's target/, so the
# two never fight over cargo's lock and repeat builds are incremental. The
# repo is mounted read-only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INST="$ROOT/installers"
DIST="$INST/dist"
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
CARGO="${CARGO:-cargo}"
REPO_SLUG="Stephen-Hilton/iter"
IMAGE="iter-build-bullseye:1"
VOL_CARGO="iter-build-cargo"
VOL_TARGET="iter-build-target"

ALL_ASSETS=(
  iter_data-linux-x86_64 iter_data-linux-aarch64
  iter_engine-linux-x86_64 iter_engine-linux-aarch64
  iter_engine-macos-aarch64 iter_engine-macos-x86_64
  iter_engine-windows-x86_64.exe
)
GLIBC_MAX="2.31"

say()  { printf '\n\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '    \033[33m!!\033[0m  %s\n' "$*" >&2; }
die()  { printf '\n\033[31mstopped:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
installers/build.sh - build the iter release binaries into installers/dist/

Usage: installers/build.sh [--only <asset-glob>]... [--skip-windows] [--dry-run] [--release]

  --only <glob>    build only assets matching the glob (repeatable), e.g.
                   --only 'iter_engine-macos-*' --only 'iter_data-linux-*'
  --skip-windows   do not build iter_engine-windows-x86_64.exe
  --dry-run        print every action, change nothing
  --release        build all 7 assets, then create the next v<YYYY>.<MM>.<DD>[.N]
                   tag at HEAD (UTC), push it, and `gh release create` with dist/*.
                   Refuses when the working tree is dirty or HEAD is not on origin.
  -h, --help       this text

Assets: iter_data-linux-{x86_64,aarch64}, iter_engine-linux-{x86_64,aarch64},
        iter_engine-macos-{aarch64,x86_64}, iter_engine-windows-x86_64.exe
Output: installers/dist/<asset> + SHA256SUMS, copied into installers/<prog>/<target>/.
EOF
}

ONLY=() SKIP_WIN=0 DRY=0 RELEASE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --only) [ $# -ge 2 ] || die "--only needs a glob"; ONLY+=("$2"); shift 2 ;;
    --only=*) ONLY+=("${1#--only=}"); shift ;;
    --skip-windows) SKIP_WIN=1; shift ;;
    --dry-run) DRY=1; shift ;;
    --release) RELEASE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done

# run CMD... : print it; execute unless --dry-run
run() {
  printf '    $'; printf ' %q' "$@"; printf '\n'
  [ "$DRY" = 1 ] || "$@"
}

selected() { # selected ASSET -> 0 when this run builds it
  local a="$1" g
  [ "$SKIP_WIN" = 1 ] && [[ "$a" == *windows* ]] && return 1
  [ ${#ONLY[@]} -eq 0 ] && return 0
  # shellcheck disable=SC2053  # $g is a glob on purpose (--only 'iter_engine-macos-*')
  for g in "${ONLY[@]}"; do [[ "$a" == $g ]] && return 0; done
  return 1
}

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# put SRC DEST : copy via a temp name in the same dir + mv (never cp over a
# file someone may be running)
put() {
  local src="$1" dest="$2" tmp
  if [ "$DRY" = 1 ]; then printf '    copy %s -> %s\n' "${src#$ROOT/}" "${dest#$ROOT/}"; return; fi
  mkdir -p "$(dirname "$dest")"
  tmp="$(dirname "$dest")/.$(basename "$dest").tmp.$$"
  cp "$src" "$tmp"; chmod 755 "$tmp"; mv -f "$tmp" "$dest"
}

# ---------------------------------------------------------------- --release checks
if [ "$RELEASE" = 1 ]; then
  [ ${#ONLY[@]} -eq 0 ] || die "--release builds every asset; drop --only"
  [ "$SKIP_WIN" = 0 ] || die "--release needs the Windows asset; drop --skip-windows"
  command -v gh >/dev/null || die "--release needs the gh CLI"
  say "release checks"
  [ -z "$(git -C "$ROOT" status --porcelain)" ] || die "working tree is dirty (git status); commit or stash first"
  git -C "$ROOT" fetch --quiet --tags origin
  HEAD_SHA="$(git -C "$ROOT" rev-parse HEAD)"
  [ -n "$(git -C "$ROOT" branch -r --contains "$HEAD_SHA" 2>/dev/null | grep '^ *origin/')" ] \
    || die "HEAD $HEAD_SHA is not on origin; push it first"
  BASE_TAG="v$(date -u +%Y.%m.%d)"
  TAG="$BASE_TAG" n=1
  while git -C "$ROOT" rev-parse -q --verify "refs/tags/$TAG" >/dev/null \
     || git -C "$ROOT" ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; do
    n=$((n + 1)); TAG="$BASE_TAG.$n"
  done
  echo "    HEAD $HEAD_SHA is on origin; the release tag will be $TAG"
fi

[ "$DRY" = 1 ] || mkdir -p "$DIST"
BUILT=() FAILED=() SKIPPED=()

# ---------------------------------------------------------------- macOS
build_macos() {
  local a
  if selected iter_engine-macos-aarch64; then
    say "iter_engine-macos-aarch64 (native; both packages, as for any native build)"
    if run "$CARGO" build --release --locked --manifest-path "$ROOT/Cargo.toml" -p iter_engine -p iter_data; then
      [ "$DRY" = 1 ] || cp "$ROOT/target/release/iter_engine" "$DIST/iter_engine-macos-aarch64"
      BUILT+=(iter_engine-macos-aarch64)
    else FAILED+=(iter_engine-macos-aarch64); fi
  fi
  if selected iter_engine-macos-x86_64; then
    say "iter_engine-macos-x86_64 (cargo --target x86_64-apple-darwin)"
    a=ok
    rustup target list --installed 2>/dev/null | grep -qx x86_64-apple-darwin \
      || run rustup target add x86_64-apple-darwin || a=fail
    if [ "$a" = ok ] && run "$CARGO" build --release --locked --manifest-path "$ROOT/Cargo.toml" \
         --target x86_64-apple-darwin -p iter_engine; then
      [ "$DRY" = 1 ] || cp "$ROOT/target/x86_64-apple-darwin/release/iter_engine" "$DIST/iter_engine-macos-x86_64"
      BUILT+=(iter_engine-macos-x86_64)
    else FAILED+=(iter_engine-macos-x86_64); fi
  fi
}

# ---------------------------------------------------------------- Linux
DOCKERFILE='FROM rust:1-bullseye
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu g++-x86-64-linux-gnu libc6-dev-amd64-cross binutils \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar \
    CARGO_TARGET_DIR=/target \
    CARGO_TERM_COLOR=always'

ensure_image() {
  if docker image inspect "$IMAGE" >/dev/null 2>&1; then echo "    build image $IMAGE present"; return; fi
  echo "    building image $IMAGE (rust:1-bullseye + x86_64 cross gcc)"
  if [ "$DRY" = 1 ]; then printf '    $ docker build --platform linux/arm64 -t %s - <<Dockerfile\n' "$IMAGE"; return; fi
  printf '%s\n' "$DOCKERFILE" | docker build --platform linux/arm64 -t "$IMAGE" -
}

build_linux() {
  local want=() a arch prog triple pkgs="" script
  for a in iter_data-linux-x86_64 iter_data-linux-aarch64 iter_engine-linux-x86_64 iter_engine-linux-aarch64; do
    selected "$a" && want+=("$a")
  done
  [ ${#want[@]} -gt 0 ] || return 0
  say "Linux (${want[*]}) in $IMAGE, glibc 2.31 baseline"
  docker info >/dev/null 2>&1 || { warn "docker is not running"; FAILED+=("${want[@]}"); return 0; }
  ensure_image
  script="set -e"
  for arch in aarch64 x86_64; do
    pkgs=""
    for prog in iter_data iter_engine; do
      [[ " ${want[*]} " == *" $prog-linux-$arch "* ]] && pkgs="$pkgs -p $prog"
    done
    [ -n "$pkgs" ] || continue
    triple="$arch-unknown-linux-gnu"
    script="$script
echo '--- cargo build --release --locked --target $triple$pkgs'
cargo build --release --locked --target $triple$pkgs"
    for prog in iter_data iter_engine; do
      [[ " ${want[*]} " == *" $prog-linux-$arch "* ]] || continue
      script="$script
cp /target/$triple/release/$prog /out/.$prog-linux-$arch.tmp && mv -f /out/.$prog-linux-$arch.tmp /out/$prog-linux-$arch"
    done
  done
  if run docker run --rm --platform linux/arm64 \
       -v "$ROOT:/src:ro" -v "$DIST:/out" \
       -v "$VOL_CARGO:/usr/local/cargo/registry" -v "$VOL_TARGET:/target" \
       -w /src "$IMAGE" bash -c "$script"; then
    BUILT+=("${want[@]}")
  else
    FAILED+=("${want[@]}")
  fi
}

# ---------------------------------------------------------------- Windows
build_windows() {
  local a=iter_engine-windows-x86_64.exe
  selected "$a" || return 0
  say "$a (cargo xwin, x86_64-pc-windows-msvc)"
  if ! "$CARGO" xwin --version >/dev/null 2>&1 || ! command -v clang-cl >/dev/null 2>&1; then
    warn "cargo-xwin or LLVM missing (cargo install --locked cargo-xwin; brew install llvm)"
    warn "skipped: install cargo-xwin (cargo install --locked cargo-xwin) and llvm (brew install llvm), or build $a on a Windows machine from a clone: cargo build --release -p iter_engine"
    SKIPPED+=("$a"); return 0
  fi
  rustup target list --installed 2>/dev/null | grep -qx x86_64-pc-windows-msvc \
    || run rustup target add x86_64-pc-windows-msvc || { FAILED+=("$a"); return 0; }
  # static CRT: no VCRUNTIME140.dll / VC++ redistributable needed on the target
  export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C target-feature=+crt-static"
  if run "$CARGO" xwin build --release --locked --manifest-path "$ROOT/Cargo.toml" \
       --target x86_64-pc-windows-msvc -p iter_engine; then
    [ "$DRY" = 1 ] || cp "$ROOT/target/x86_64-pc-windows-msvc/release/iter_engine.exe" "$DIST/$a"
    BUILT+=("$a")
  else
    warn "failed: install cargo-xwin (cargo install --locked cargo-xwin) and llvm (brew install llvm), or build $a on a Windows machine from a clone: cargo build --release -p iter_engine"
    FAILED+=("$a")
  fi
}

build_macos
build_linux
build_windows

# ---------------------------------------------------------------- SHA256SUMS
say "SHA256SUMS"
if [ "$DRY" = 1 ]; then
  echo "    write installers/dist/SHA256SUMS for every asset in dist/"
else
  : > "$DIST/.SHA256SUMS.tmp"
  for a in "${ALL_ASSETS[@]}"; do
    [ -f "$DIST/$a" ] && printf '%s  %s\n' "$(sha "$DIST/$a")" "$a" >> "$DIST/.SHA256SUMS.tmp"
  done
  mv -f "$DIST/.SHA256SUMS.tmp" "$DIST/SHA256SUMS"
  sed 's/^/    /' "$DIST/SHA256SUMS"
fi

# ---------------------------------------------------------------- place into folders
say "copy into the installer folders"
folders_for() { # folders_for ASSET -> installer folders (relative to installers/)
  case "$1" in
    iter_data-linux-*)   echo "iter_data/docker iter_data/linux iter_data/linux_aws" ;;
    iter_engine-linux-*) echo "iter_engine/docker iter_engine/linux iter_engine/linux_wsl" ;;
    iter_engine-macos-*) echo "iter_engine/macos" ;;
    iter_engine-windows-*) echo "iter_engine/windows" ;;
  esac
}
for a in ${BUILT[@]+"${BUILT[@]}"}; do
  for f in $(folders_for "$a"); do put "$DIST/$a" "$INST/$f/$a"; done
done

# ---------------------------------------------------------------- summary
glibc_report() { # -> lines "asset glibc" for the Linux binaries in dist/
  local list=() a
  for a in iter_data-linux-x86_64 iter_data-linux-aarch64 iter_engine-linux-x86_64 iter_engine-linux-aarch64; do
    [ -f "$DIST/$a" ] && list+=("$a")
  done
  [ ${#list[@]} -gt 0 ] || return 0
  docker image inspect "$IMAGE" >/dev/null 2>&1 || return 0
  docker run --rm --platform linux/arm64 -v "$DIST:/out:ro" "$IMAGE" bash -c '
    for a in "$@"; do
      v=$(readelf --dyn-syms -W "/out/$a" | grep -o "GLIBC_[0-9][0-9.]*" | sed "s/GLIBC_//" | sort -uV | tail -1)
      echo "$a ${v:-none}"
    done' _ "${list[@]}"
}

say "summary"
if [ "$DRY" = 1 ]; then
  echo "    (dry run: nothing built)"
  echo "    would build: $(for a in "${ALL_ASSETS[@]}"; do selected "$a" && printf '%s ' "$a"; done)"
  exit 0
fi
GLIBC="$(glibc_report || true)"
BAD_GLIBC=0
printf '    %-32s %9s  %-12s %-6s %s\n' asset size sha256 glibc status
for a in "${ALL_ASSETS[@]}"; do
  st="-"
  [[ " ${BUILT[*]-} " == *" $a "* ]] && st=built
  [[ " ${FAILED[*]-} " == *" $a "* ]] && st=FAILED
  [[ " ${SKIPPED[*]-} " == *" $a "* ]] && st=skipped
  if [ -f "$DIST/$a" ]; then
    size="$(du -h "$DIST/$a" | cut -f1)"; s="$(sha "$DIST/$a" | cut -c1-12)"
    g="$(printf '%s\n' "$GLIBC" | awk -v a="$a" '$1==a {print $2}')"; g="${g:-n/a}"
    if [ "$g" != n/a ] && [ "$g" != none ] && \
       [ "$(printf '%s\n%s\n' "$g" "$GLIBC_MAX" | sort -V | tail -1)" != "$GLIBC_MAX" ]; then
      g="$g>$GLIBC_MAX!"; BAD_GLIBC=1
    fi
    [ "$st" = "-" ] && st="(old copy)"
  else size="-"; s="-"; g="-"; fi
  printf '    %-32s %9s  %-12s %-6s %s\n' "$a" "$size" "$s" "$g" "$st"
done
[ "$BAD_GLIBC" = 0 ] || warn "a Linux binary needs glibc newer than $GLIBC_MAX"

# ---------------------------------------------------------------- --release publish
if [ "$RELEASE" = 1 ]; then
  [ ${#FAILED[@]} -eq 0 ] && [ ${#SKIPPED[@]} -eq 0 ] || die "not releasing: ${FAILED[*]-} ${SKIPPED[*]-} did not build"
  [ "$BAD_GLIBC" = 0 ] || die "not releasing: glibc baseline exceeded"
  for a in "${ALL_ASSETS[@]}"; do [ -f "$DIST/$a" ] || die "not releasing: dist/$a missing"; done
  [ -z "$(git -C "$ROOT" status --porcelain)" ] || die "working tree changed during the build"
  [ "$(git -C "$ROOT" rev-parse HEAD)" = "$HEAD_SHA" ] || die "HEAD moved during the build"
  say "release $TAG"
  NOTES="$DIST/.release-notes.md"
  {
    echo "iter release $TAG, built from commit $HEAD_SHA ($(git -C "$ROOT" log -1 --format=%s "$HEAD_SHA"))."
    echo
    echo "Install with the scripts under installers/ (see installers/README.md); pass \`--version $TAG\` to pin this release."
    echo
    echo "| asset | size | sha256 |"
    echo "|---|---|---|"
    for a in "${ALL_ASSETS[@]}"; do
      echo "| $a | $(du -h "$DIST/$a" | cut -f1) | \`$(sha "$DIST/$a")\` |"
    done
  } > "$NOTES"
  run git -C "$ROOT" tag -a "$TAG" -m "iter $TAG" "$HEAD_SHA"
  run git -C "$ROOT" push origin "refs/tags/$TAG"
  ASSET_PATHS=(); for a in "${ALL_ASSETS[@]}"; do ASSET_PATHS+=("$DIST/$a"); done
  run gh release create "$TAG" --repo "$REPO_SLUG" --title "iter $TAG" --notes-file "$NOTES" \
      --verify-tag "${ASSET_PATHS[@]}" "$DIST/SHA256SUMS"
  rm -f "$NOTES"
  echo "    https://github.com/$REPO_SLUG/releases/tag/$TAG"
fi

[ ${#FAILED[@]} -eq 0 ] || exit 1
