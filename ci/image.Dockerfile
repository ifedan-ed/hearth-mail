# Hearth Mail server image.
#
# The binary is built beforehand by ci/build-image.sh in the pinned toolchain
# container (ci/builder.Dockerfile), where memory, CPUs and jobs are capped;
# a `docker build` can't be. This file only lays the image out, the same way
# upstream's image does, so the live compose file (user 2000, /etc/inbuxa,
# /var/lib/inbuxa, the ports, the health check) works unchanged.
#
# Build context: the directory ci/build-image.sh fills (dist/), holding
# `inbuxa` (the server binary).

# inbuxa-migrate, the account import/export tool the image has always
# carried (README, "Copying mail"): the release upstream's 27.1-rc.39 image
# pins, checked against its checksum.
FROM docker.io/library/debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS migrate
ARG MIGRATE_VERSION=27.1-rc.16
ARG MIGRATE_SHA256=8ba07fe60ab0acc8d7c8d7b7ba14fbbe7173955777fbfeee93c2d0d73ae22160
RUN export DEBIAN_FRONTEND=noninteractive && \
    apt-get update && \
    apt-get install -yq --no-install-recommends ca-certificates curl && \
    curl -fsSL -o /migrate.tar.gz "https://github.com/inbuxa/inbuxa-migrate/releases/download/v${MIGRATE_VERSION}/inbuxa-migrate-linux-amd64.tar.gz" && \
    echo "${MIGRATE_SHA256}  /migrate.tar.gz" | sha256sum -c - && \
    mkdir /migrate && tar -xzf /migrate.tar.gz -C /migrate

FROM docker.io/library/debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
ARG REVISION=""
LABEL org.opencontainers.image.title="Hearth Mail" \
      org.opencontainers.image.source="https://github.com/ifedan-ed/hearth-mail" \
      org.opencontainers.image.revision="${REVISION}"
RUN export DEBIAN_FRONTEND=noninteractive && \
    apt-get update && \
    apt-get install -yq --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd -r -g 2000 inbuxa && \
    useradd -r -u 2000 -g 2000 -s /usr/sbin/nologin -M inbuxa && \
    mkdir -p /etc/inbuxa /var/lib/inbuxa && \
    chown inbuxa:inbuxa /etc/inbuxa /var/lib/inbuxa
COPY --chmod=0755 inbuxa /usr/local/bin/inbuxa
COPY --from=migrate --chmod=0755 /migrate/inbuxa-migrate /usr/local/bin/inbuxa-migrate
COPY --from=migrate /migrate/LICENSES /usr/share/doc/inbuxa-migrate/LICENSES
COPY --from=migrate /migrate/README.md /usr/share/doc/inbuxa-migrate/README.md
USER inbuxa
WORKDIR /var/lib/inbuxa
VOLUME ["/etc/inbuxa", "/var/lib/inbuxa"]
EXPOSE 443 25 110 587 465 143 993 995 4190 8080
ENV INBUXA_HEALTHCHECK_URL=https://127.0.0.1:443/healthz/live
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -fsSk -H "X-Forwarded-For: 127.0.0.1" "$INBUXA_HEALTHCHECK_URL" || curl -fsS -H "X-Forwarded-For: 127.0.0.1" http://127.0.0.1:8080/healthz/live || exit 1
ENTRYPOINT ["/usr/local/bin/inbuxa"]
CMD ["--config", "/etc/inbuxa/config.json"]
