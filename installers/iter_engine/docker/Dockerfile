# iter engine in a container: Debian bookworm-slim + git, bash, the claude CLI
# and the prebuilt iter_engine for this platform (no compiling). install.sh
# builds it in ~/.iter/docker with compose.yml; by hand, from a folder that
# holds this file, entrypoint.sh and iter_engine-linux-<arch>:
#   docker build -t iter-engine:local .
FROM debian:bookworm-slim

ARG TARGETARCH
# git/bash/curl/jq: the engine and its shell steps; openssh-client: ssh git
# remotes; poppler-utils: GraphRAG reads PDF pages; openssl: install.sh's own
# API calls check iter_data's pinned certificate with it; libstdc++6/libgcc-s1:
# iter_engine links them (bookworm-slim has them)
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash ca-certificates curl git jq openssh-client openssl poppler-utils procps less \
      libstdc++6 libgcc-s1 \
 && rm -rf /var/lib/apt/lists/*

# the claude CLI the agents run: Anthropic's native installer, its binary moved
# to /usr/local/bin (the image is rebuilt to update it, so no auto-updater).
# CLAUDE_VERSION pins one (e.g. 2.1.302); default: the installer's latest.
ARG CLAUDE_VERSION=
ENV DISABLE_AUTOUPDATER=1
RUN curl -fsSL https://claude.ai/install.sh | bash -s -- ${CLAUDE_VERSION} \
 && install -m 755 "$(readlink -f /root/.local/bin/claude)" /usr/local/bin/claude \
 && rm -rf /root/.local/share/claude /root/.local/bin/claude /root/.claude /root/.claude.json \
 && claude --version

COPY iter_engine-linux-* /tmp/iter/
RUN case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64|x86_64) a=x86_64 ;; arm64|aarch64) a=aarch64 ;; \
      *) echo "no iter_engine for ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
 && test -f "/tmp/iter/iter_engine-linux-$a" || { echo "iter_engine-linux-$a is not in the build context" >&2; exit 1; } \
 && install -m 755 "/tmp/iter/iter_engine-linux-$a" /usr/local/bin/iter_engine \
 && rm -rf /tmp/iter \
 && iter_engine --help >/dev/null

COPY entrypoint.sh /usr/local/bin/iter-engine-entrypoint
RUN chmod 755 /usr/local/bin/iter-engine-entrypoint

# the entrypoint drops to the host user's uid/gid with HOME = the host's home
# path (ITER_UID, ITER_GID, ITER_HOME_DIR; compose.yml sets them), so files in
# the checkouts stay the host user's and ~/dev/<project> topdirs match
ENTRYPOINT ["/usr/local/bin/iter-engine-entrypoint"]
CMD []
