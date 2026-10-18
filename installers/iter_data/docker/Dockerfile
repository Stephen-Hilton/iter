# iter_data all-in-one image: ArangoDB Community Edition 3.12 + iter_data, one container.
#
# Built by install.sh in this folder (or `docker compose up -d --build` here). No Rust
# toolchain: the image takes the PREBUILT iter_data binary that sits next to this file,
#   iter_data-linux-x86_64   (amd64 images)
#   iter_data-linux-aarch64  (arm64 images)
# chosen by Docker's TARGETARCH, plus the embedding model in models/all-MiniLM-L6-v2/.
# A repo checkout has neither: install.sh downloads the binary from the GitHub Release
# (or `installers/build.sh` builds it, or `install.sh --binary PATH` uses yours) and
# fetches the model before it builds.
#
# The runtime is Debian (glibc), because iter_data is a glibc binary; ArangoDB's own
# binaries are static, so they are copied out of the official image unchanged.

ARG ARANGO_IMAGE=arangodb:3.12.12

FROM ${ARANGO_IMAGE} AS arango

# ---- pick the binary for the image's architecture
FROM debian:bookworm-slim AS pick
ARG TARGETARCH
COPY iter_data-linux-* /in/
RUN set -e; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) a=x86_64 ;; arm64) a=aarch64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    if [ ! -f "/in/iter_data-linux-$a" ]; then \
      echo "iter_data-linux-$a is not in the build folder: run install.sh (it downloads it), installers/build.sh, or install.sh --binary PATH" >&2; \
      exit 1; \
    fi; \
    install -m 755 "/in/iter_data-linux-$a" /iter_data

# ---- runtime
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*

# ArangoDB (static binaries, its JS/ICU/tzdata share dir, its config, its entrypoint;
# the entrypoint initialises the root password on first start)
COPY --from=arango /usr/sbin/arangod /usr/sbin/arangod
COPY --from=arango /usr/bin/arangosh /usr/bin/arangodump /usr/bin/arangorestore /usr/bin/
COPY --from=arango /usr/share/arangodb3 /usr/share/arangodb3
COPY --from=arango /etc/arangodb3 /etc/arangodb3
COPY --from=arango /entrypoint.sh /usr/local/bin/arango-entrypoint.sh

COPY --from=pick /iter_data /usr/local/bin/iter_data
COPY entrypoint.sh /usr/local/bin/iter-entrypoint.sh
# GraphRAG embedding model (install.sh fetches it into models/)
COPY models/all-MiniLM-L6-v2 /opt/iter/models/all-MiniLM-L6-v2

# Everything that persists lives under one mount, /var/lib/iter_data = the host's
# <iter dir>/iter_data: arango/data (the database), arango/apps (Arango's Foxx app
# dir), state/ (iter_data's secret files, engine_session.secret, the TLS certificate
# tls.crt/tls.key and the .env file), config/ (the secrets file install.sh writes,
# a certificate of your own: custom-tls.crt/.key, an ArangoDB CA: arango-ca.crt),
# backups/. The paths arangod and its entrypoint hardcode are links into it; the
# entrypoint creates the subdirectories.
RUN chmod +x /usr/local/bin/iter-entrypoint.sh /usr/local/bin/arango-entrypoint.sh \
 && ln -s arangod /usr/sbin/arango-init-database \
 && mkdir -p /var/lib/iter_data \
 && ln -s /var/lib/iter_data/arango/data /var/lib/arangodb3 \
 && ln -s /var/lib/iter_data/arango/apps /var/lib/arangodb3-apps \
 && ln -s /var/lib/iter_data/state /var/lib/iter

# ARANGO_DB is left unset: iter_data's default is iter_db, and the entrypoint keeps
# an older install on its database iter5. ARANGO_URL elsewhere = no arangod here.
ENV ARANGO_VERSION=3.12.12 \
    GLIBCXX_FORCE_NEW=1 \
    ARANGO_URL=http://127.0.0.1:8529 \
    ITER_EMBED_MODEL=/opt/iter/models/all-MiniLM-L6-v2 \
    ITER_LISTEN=0.0.0.0:8400
VOLUME ["/var/lib/iter_data"]
EXPOSE 8400 8529
# iter_data serves HTTPS with a self-signed certificate (-k: this only asks "up?");
# with ITER_TLS=off it is plain http
HEALTHCHECK --interval=15s --timeout=5s --start-period=60s --retries=5 \
  CMD curl -fsSk https://127.0.0.1:8400/health 2>/dev/null | grep -q '"ok":true' \
   || curl -fsS http://127.0.0.1:8400/health 2>/dev/null | grep -q '"ok":true' || exit 1
ENTRYPOINT ["/usr/local/bin/iter-entrypoint.sh"]
