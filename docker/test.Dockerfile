FROM ghcr.io/astral-sh/uv:0.12.3@sha256:2d890623d310b57771ce840f0da5eed5fc6d657da05ffaa45d82797b53fa3abc AS uv
FROM python:3.13-slim-bookworm@sha256:00faa2debb87529f9f0764e9491d8ba400a3678976616c3bd7cb193745ac20d1

COPY --from=uv /uv /uvx /bin/

RUN apt-get update \
    && apt-get install --yes --no-install-recommends git libatomic1 build-essential pkg-config libssl-dev liblzma-dev patchelf nodejs \
    && rm -rf /var/lib/apt/lists/*

# Bootstrap the latest Soldr binary wheel; source builds use the repository's
# rust-toolchain.toml through this Soldr front door. Never pin an old Soldr.
RUN uv pip install --system --only-binary=:all: soldr

ENV UV_PROJECT_ENVIRONMENT=/venv \
    UV_CACHE_DIR=/root/.cache/uv \
    UV_LINK_MODE=copy \
    RUFF_CACHE_DIR=/root/.cache/ruff \
    PYRIGHT_PYTHON_CACHE_DIR=/root/.cache/pyright-python \
    CARGO_HOME=/root/.cargo \
    RUSTUP_HOME=/root/.rustup \
    CARGO_TARGET_DIR=/target \
    PYTHONDONTWRITEBYTECODE=1

# zackees/ci.yml GATE-005: the test guards (tests/conftest.py,
# ci/test_guard.sh) let bosn's suites run here and refuse a bare host.
ENV BOSN_TEST_ISOLATED=1

WORKDIR /repo
