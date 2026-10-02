# Multi-stage build: shared source, release builder, test runner, and non-root runtime.
# Toolchains, base-image digests, and Cargo.lock pin the build inputs.
# Build arguments may select mirrors of the same image digest.

ARG RUST_BUILDER_IMAGE=rust:1.98.1-slim-trixie@sha256:ce84a5edd80c5f91e05c5533b1e53eb1da54028f33734dc06aa6b49fa190462d
ARG RUNTIME_IMAGE=debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

# ---------------------------------------------------------------------------
FROM ${RUST_BUILDER_IMAGE} AS source

# 离线编译固定 SQLx 查询；限制并行度，避免 8 GiB 宿主在构建时连同现有服务一起过载。
ARG CARGO_BUILD_JOBS=2
ENV SQLX_OFFLINE=true \
    CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_TERM_COLOR=never

WORKDIR /app
# 只复制构建/测试真正需要的文件，不从上下文带入 uploads/.env/target/备份。
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
COPY src ./src
COPY queries ./queries
COPY migrations ./migrations
COPY .sqlx ./.sqlx
COPY tests ./tests
COPY scripts ./scripts

# ---------------------------------------------------------------------------
FROM source AS builder

# 依赖下载只写入构建缓存挂载，不进入最终镜像层；`--locked` 严格按 Cargo.lock 构建。
# `/app/target` 挂载为专用 BuildKit 缓存供后续增量编译（RUN 结束即卸载、不进入镜像层），
# 因此同一步把 release 二进制复制到缓存之外的 `/out`，供 runtime 阶段 COPY。
RUN --mount=type=cache,id=lycoris-rust-release-builder-target,target=/app/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --locked --bin lycoris-backend \
    && install -D -m 0755 /app/target/release/lycoris-backend /out/lycoris-backend

# ---------------------------------------------------------------------------
FROM source AS test

# 测试并发上限：共享 1 GiB 合成 PG，另有并行工作树在跑迁移测试，保持 2。
ENV RUST_TEST_THREADS=2

# 容器内回环转发器（标准库实现，rustc 直接编译，无第三方依赖、无 apt）。
RUN rustc -O /app/scripts/tcp-forward.rs -o /usr/local/bin/tcp-forward

# 运行器脚本只连接已授权的合成服务，并把它们转发到本容器回环以满足 tests/common 的
# 回环校验；不连接 restore-review/生产地址。
ENTRYPOINT ["/bin/bash", "/app/scripts/run-linux-tests.sh"]

# ---------------------------------------------------------------------------
FROM ${RUNTIME_IMAGE} AS runtime

ARG APP_UID=10001
ARG APP_GID=10001

# 最小运行依赖：仅 glibc/libgcc-s1（Debian 基础即含）与 CA 证书。debian slim 不预置
# ca-certificates，从带 HTTPS 工具链的 builder 复制系统 CA bundle，避免 runtime 再装包。
RUN set -eux; \
    groupadd --gid "${APP_GID}" lycoris; \
    useradd --uid "${APP_UID}" --gid "${APP_GID}" --no-create-home \
        --home-dir /nonexistent --shell /usr/sbin/nologin lycoris; \
    install -d -o "${APP_UID}" -g "${APP_GID}" -m 0750 /var/lib/lycoris/uploads; \
    mkdir -p /etc/ssl/certs

COPY --from=builder /out/lycoris-backend /usr/local/bin/lycoris-backend
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# 监听与目录默认值：容器内必须监听所有接口；上传目录为持久卷挂载点。
# 临时目录统一为 `/tmp`，由运行时 tmpfs 提供（read_only 根下仍可写）。
ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=18081 \
    UPLOAD_DIR=/var/lib/lycoris/uploads \
    TMPDIR=/tmp \
    RUST_LOG=info

EXPOSE 18081

# 非 root 固定 UID/GID；只读根文件系统由 compose/运行时施加。
USER ${APP_UID}:${APP_GID}

# 真实可执行的健康检查：对 /health/ready 发起一次 HTTP GET，2xx 才健康。
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=6 \
    CMD ["/usr/local/bin/lycoris-backend", "--healthcheck"]

# 不自动迁移、不接管已有库：启动参数需显式传入（例如 `--migrate` 仅由演练者手动调用）。
ENTRYPOINT ["/usr/local/bin/lycoris-backend"]
