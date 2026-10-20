# syntax=docker/dockerfile:1

# ---- Build: static musl binary -------------------------------------------
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app

# Build dependencies first so they are cached across source changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY templates ./templates
COPY static ./static
COPY examples ./examples
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# ---- Runtime -------------------------------------------------------------
FROM alpine:3

RUN addgroup -S cv && adduser -S -G cv -h /data cv \
    && mkdir -p /data && chown cv:cv /data
COPY --from=builder /app/target/release/cvgenerator /usr/local/bin/cvgenerator

USER cv
ENV CV_BIND=0.0.0.0:8080 \
    CV_DATA_DIR=/data \
    RUST_LOG=info
VOLUME ["/data"]
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD wget -qO- http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["cvgenerator"]
CMD ["serve"]
