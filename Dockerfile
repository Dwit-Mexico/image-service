# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /app

# Dependencias nativas para webp
RUN apt-get update && apt-get install -y libwebp-dev && rm -rf /var/lib/apt/lists/*

# Cache de dependencias (solo Cargo.toml/lock + stubs de bins/lib)
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && \
    echo 'fn main(){}' > src/main.rs && \
    echo 'fn main(){}' > src/bin/migrate.rs && \
    echo 'fn main(){}' > src/bin/seed_from_env.rs && \
    echo 'fn main(){}' > src/bin/project_admin.rs && \
    echo '' > src/lib.rs
RUN cargo build --release
RUN rm -rf src

# Código fuente real + migraciones + metadata sqlx (compila offline, sin DB)
COPY src ./src
COPY migrations ./migrations
COPY .sqlx ./.sqlx
ENV SQLX_OFFLINE=true
# Toca mtimes para forzar a Cargo a recompilar: Docker COPY preserva mtimes
# del host (anteriores al stub build), así que sin touch Cargo cree que los
# binarios están up-to-date y deja los stubs en target/release.
RUN find src -name '*.rs' -exec touch {} + && cargo build --release

# ── Runner ────────────────────────────────────────────────────────────────────
FROM debian:trixie-slim AS runner

RUN apt-get update \
    && apt-get install -y --no-install-recommends libwebp7 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/image-service .
COPY --from=builder /app/target/release/migrate .
COPY --from=builder /app/target/release/seed-from-env .
COPY --from=builder /app/target/release/project-admin .

EXPOSE 8080
ENV LISTEN_ADDR=0.0.0.0:8080

CMD ["/app/image-service"]
