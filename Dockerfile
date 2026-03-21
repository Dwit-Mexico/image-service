# ── Builder ──────────────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /app

# Dependencias nativas para webp
RUN apt-get update && apt-get install -y libwebp-dev && rm -rf /var/lib/apt/lists/*

# Cache de dependencias (solo Cargo.toml/lock)
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs
RUN cargo build --release
RUN rm src/main.rs

# Código fuente
COPY src ./src
# Forzar recompilación del binario
RUN touch src/main.rs && cargo build --release

# ── Runner ────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runner

RUN apt-get update \
    && apt-get install -y --no-install-recommends libwebp7 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/image-service .

EXPOSE 8080
ENV LISTEN_ADDR=0.0.0.0:8080

CMD ["./image-service"]
