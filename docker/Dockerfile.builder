FROM 47.97.67.233:5000/deepshield/rust-build:cached AS base

WORKDIR /build

# Use git CLI for fetching to avoid libgit2 SSL issues
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

# ── Upgrade Rust from 1.83.0 → 1.96.0 (must match rust-toolchain.toml) ──
# rustup can't update in-place on overlayfs (cross-device link error), so we
# copy to the writable layer first, update there, then swap back.
# Install with profile=minimal + required components to match rust-toolchain.toml
# exactly, so cargo won't trigger any downloads at runtime.
RUN cp -a /usr/local/rustup /tmp/rustup && \
    cp -a /usr/local/cargo /tmp/cargo && \
    RUSTUP_HOME=/tmp/rustup CARGO_HOME=/tmp/cargo rustup toolchain install 1.96.0 \
        --profile minimal \
        --component rustfmt \
        --component clippy \
        --component rust-src && \
    RUSTUP_HOME=/tmp/rustup CARGO_HOME=/tmp/cargo rustup default 1.96.0 && \
    rm -rf /usr/local/rustup /usr/local/cargo && \
    mv /tmp/rustup /usr/local/rustup && \
    mv /tmp/cargo /usr/local/cargo

# ── Install cargo-chef for dependency caching ──
# 0.1.77 supports edition = "2024"; pre-installed 0.1.68 does not.
RUN cargo install cargo-chef --version 0.1.77 --locked

# ==============================================================================
# Stage 1: Analyze workspace structure
FROM base AS planner
COPY . /build/zerotrace
WORKDIR /build/zerotrace
RUN cargo chef prepare --recipe-path recipe.json

# ==============================================================================
# Stage 2: Pre-compile all dependencies
FROM base AS cacher
WORKDIR /build/zerotrace
COPY --from=planner /build/zerotrace/recipe.json recipe.json
RUN cargo chef cook --recipe-path recipe.json

# ==============================================================================
# Stage 3: Final builder image with pre-compiled dependency cache
FROM base AS builder
WORKDIR /zerotrace

COPY --from=cacher /usr/local/cargo/registry /usr/local/cargo/registry
COPY --from=cacher /usr/local/cargo/git /usr/local/cargo/git
COPY --from=cacher /build/zerotrace/target /zerotrace/target

CMD ["bash"]
