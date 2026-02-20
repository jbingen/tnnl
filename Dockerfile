FROM rust:1.85-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/tnnl /usr/local/bin/tnnl
EXPOSE 8080 9443
CMD ["sh", "-c", "exec tnnl server --domain ${DOMAIN} --http-port 8080 --control-port 9443 ${TOKEN:+--token ${TOKEN}}"]
