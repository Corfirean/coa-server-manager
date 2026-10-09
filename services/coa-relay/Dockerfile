# Build context: the repository root.   docker build -f services/coa-relay/Dockerfile -t coa-relay:<tag> .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY crates/coa-registry-proto crates/coa-registry-proto
COPY crates/coa-control-proto crates/coa-control-proto
COPY services/coa-relay services/coa-relay
WORKDIR /src/services/coa-relay
RUN cargo build --release --locked --bin coa-relay

FROM debian:bookworm-slim
RUN useradd --system --uid 10002 --user-group --no-create-home --shell /usr/sbin/nologin coa
COPY --from=build /src/services/coa-relay/target/release/coa-relay /usr/local/bin/coa-relay
USER 10002:10002
EXPOSE 8082
EXPOSE 40000-40050
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/coa-relay", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/coa-relay"]
