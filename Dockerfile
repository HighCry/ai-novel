FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
COPY assets ./assets
RUN cargo build --release --bin ai-novel

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/ai-novel /usr/local/bin/ai-novel
ENV AI_NOVEL_HOST=0.0.0.0 \
    AI_NOVEL_PORT=8686 \
    AI_NOVEL_DATA=/data \
    AI_NOVEL_NO_BROWSER=1
VOLUME /data
EXPOSE 8686
CMD ["ai-novel"]
