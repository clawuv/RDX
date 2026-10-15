//! rd-server 二进制：环境变量见 `config_from_env`；WS 端点为 `/ws`，
//! 健康检查 `/healthz`。生产部署（caddy TLS + coturn + Postgres）见
//! deploy/README.md。

fn main() {
    let config = rd_server::config_from_env();
    let addr = std::env::var("RDX_SERVER_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|error| panic!("bind {addr}: {error}"));
        eprintln!(
            "rd-server listening on ws://{addr}/ws (turn {} stun {})",
            config.turn_addr, config.stun_addr
        );
        rd_server::serve(listener, config)
            .await
            .expect("signaling server");
    });
}
