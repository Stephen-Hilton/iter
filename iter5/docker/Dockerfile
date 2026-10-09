# iter5 all-in-one: ArangoDB Community Edition + iter_data in one container.
# Build from the iter5/ directory:  docker build -f docker/Dockerfile -t iter5 .
# (or `docker compose -f docker/compose.yml up -d --build`, or `./deploy.sh docker`)

# ---- build: a static musl iter_data (runs on the Alpine-based arangodb image)
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev gcc g++ make cmake perl clang linux-headers
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY iter_core iter_core
COPY iter_data iter_data
COPY iter_engine iter_engine
COPY iter_local iter_local
COPY iter_rag iter_rag
# the user guide GraphRAG ingests (only iter4_guide.md passes .dockerignore)
COPY docs docs
COPY webui webui
COPY tools/iter_engine_setup.sh tools/iter_engine_setup.sh
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p iter_data && cp target/release/iter_data /iter_data

# ---- runtime: the official ArangoDB image (Alpine, CE licence terms)
FROM arangodb:3.12 AS arango
COPY --from=build /iter_data /usr/local/bin/iter_data
COPY docker/iter5-entrypoint.sh /usr/local/bin/iter5-entrypoint.sh
# GraphRAG embedding model (tools/fetch_model.sh; ./deploy.sh docker fetches it)
COPY models/all-MiniLM-L6-v2 /opt/iter/models/all-MiniLM-L6-v2
# Everything that persists lives under one mount, /var/lib/iter_data:
# arango/ (the database), arango-apps/ (Arango's Foxx app dir), iter/
# (iter_data's secret and .env files). The paths arangod and its entrypoint
# hardcode are links into it; the entrypoint creates the subdirectories.
RUN chmod +x /usr/local/bin/iter5-entrypoint.sh \
 && rm -rf /var/lib/arangodb3 /var/lib/arangodb3-apps /var/lib/iter \
 && mkdir -p /var/lib/iter_data \
 && ln -s /var/lib/iter_data/arango /var/lib/arangodb3 \
 && ln -s /var/lib/iter_data/arango-apps /var/lib/arangodb3-apps \
 && ln -s /var/lib/iter_data/iter /var/lib/iter

# Flatten: arangodb:3.12 declares VOLUME /var/lib/arangodb3 and
# /var/lib/arangodb3-apps, which a Dockerfile can't undeclare, and Docker would
# give each an anonymous volume beside ours. Copying the filesystem into a
# fresh stage drops that metadata, so the image's ENV is restated here.
FROM scratch
COPY --from=arango / /
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    ARANGO_VERSION=3.12.12 \
    GLIBCXX_FORCE_NEW=1 \
    ARANGO_URL=http://127.0.0.1:8529 \
    ITER_EMBED_MODEL=/opt/iter/models/all-MiniLM-L6-v2 \
    ARANGO_DB=iter5 \
    ITER_LISTEN=0.0.0.0:8400
VOLUME ["/var/lib/iter_data"]
EXPOSE 8400 8529
HEALTHCHECK --interval=15s --timeout=5s --start-period=60s --retries=5 \
  CMD wget -qO- http://127.0.0.1:8400/health | grep -q '"ok":true' || exit 1
ENTRYPOINT ["/usr/local/bin/iter5-entrypoint.sh"]
