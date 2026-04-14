FROM rust:1.89-slim AS builder

WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 appuser

WORKDIR /app
COPY --from=builder /app/target/release/uchimachi-dashboard /usr/local/bin/uchimachi-dashboard

ENV RUST_LOG=info
EXPOSE 8080

USER appuser
CMD ["uchimachi-dashboard"]