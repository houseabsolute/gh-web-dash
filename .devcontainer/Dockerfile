FROM rust:latest

RUN apt-get update \
    && apt-get install -y curl git \
    && rm -rf /var/lib/apt/lists/*

# The app authenticates with `gh auth token`, so the CLI has to be here for
# the container to be able to run it at all. Run `gh auth login` once inside.
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y gh \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy

ARG MISE_VERSION=v2026.8.2
ARG MISE_SHA256=9a6c2a8b6fe77c98f1d210b4b3cbd2a0cf31886ad1baf10698757a3234c678c1
RUN curl -fsSL "https://github.com/jdx/mise/releases/download/${MISE_VERSION}/mise-${MISE_VERSION}-linux-x64" \
        -o /usr/local/bin/mise \
    && echo "${MISE_SHA256}  /usr/local/bin/mise" | sha256sum -c \
    && chmod +x /usr/local/bin/mise

RUN useradd -m -s /bin/bash dev \
    && mkdir -p /workspace \
    && chown dev:dev /workspace

USER dev

# Created here, as dev, so the named volume mounted over it in
# devcontainer.json inherits this ownership. Docker seeds a fresh named volume
# from the image's contents at that path; if the directory did not exist, the
# volume would be root-owned and gh could not write to it.
RUN mkdir -p /home/dev/.config/gh

RUN echo 'eval "$(mise activate bash)"' >> /home/dev/.bashrc
