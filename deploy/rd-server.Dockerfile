# 多阶段构建：workspace 内构建 rd-server 二进制并打进最小运行镜像。
FROM rust:1.95-slim AS build
WORKDIR /rdx
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
# deny.toml 不影响构建，仅本地审查用。
RUN cargo build --release -p rd-server --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /rdx/target/release/rd-server /usr/local/bin/rd-server
EXPOSE 8080
ENTRYPOINT ["rd-server"]
