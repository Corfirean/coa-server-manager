# Build context: the repository root.   docker build -f services/coa-registry/Dockerfile -t coa-registry:<tag> .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY crates/coa-registry-proto crates/coa-registry-proto
COPY services/coa-registry services/coa-registry
WORKDIR /src/services/coa-registry
RUN cargo build --release --locked --bin coa-registry

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin coa
COPY --from=build /src/services/coa-registry/target/release/coa-registry /usr/local/bin/coa-registry
USER 10001:10001
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/coa-registry", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/coa-registry"]
