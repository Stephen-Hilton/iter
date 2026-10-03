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
FROM arangodb:3.12
COPY --from=build /iter_data /usr/local/bin/iter_data
COPY docker/iter5-entrypoint.sh /usr/local/bin/iter5-entrypoint.sh
# GraphRAG embedding model (tools/fetch_model.sh; ./deploy.sh docker fetches it)
COPY models/all-MiniLM-L6-v2 /opt/iter/models/all-MiniLM-L6-v2
RUN chmod +x /usr/local/bin/iter5-entrypoint.sh && mkdir -p /var/lib/iter
ENV ARANGO_URL=http://127.0.0.1:8529 \
    ITER_EMBED_MODEL=/opt/iter/models/all-MiniLM-L6-v2 \
    ARANGO_DB=iter5 \
    ITER_LISTEN=0.0.0.0:8400
VOLUME ["/var/lib/arangodb3", "/var/lib/iter"]
EXPOSE 8400 8529
HEALTHCHECK --interval=15s --timeout=5s --start-period=60s --retries=5 \
  CMD wget -qO- http://127.0.0.1:8400/health | grep -q '"ok":true' || exit 1
ENTRYPOINT ["/usr/local/bin/iter5-entrypoint.sh"]
