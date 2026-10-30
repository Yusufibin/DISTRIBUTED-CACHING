FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release --locked && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cache-node /usr/local/bin/cache-node
RUN useradd -ms /bin/bash cache && mkdir -p /data && chown cache:cache /data
USER cache
EXPOSE 8080 9090
VOLUME ["/data"]
ENTRYPOINT ["cache-node"]
CMD ["--help"]
