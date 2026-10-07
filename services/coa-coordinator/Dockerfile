# Build context: the repository root.   docker build -f services/coa-coordinator/Dockerfile -t coa-coordinator:<tag> .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY crates/coa-registry-proto crates/coa-registry-proto
COPY crates/coa-control-proto crates/coa-control-proto
COPY services/coa-coordinator services/coa-coordinator
WORKDIR /src/services/coa-coordinator
RUN cargo build --release --locked --bin coa-coordinator

FROM debian:bookworm-slim
RUN useradd --system --uid 10002 --user-group --no-create-home --shell /usr/sbin/nologin coa
COPY --from=build /src/services/coa-coordinator/target/release/coa-coordinator /usr/local/bin/coa-coordinator
USER 10002:10002
EXPOSE 8081
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/coa-coordinator", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/coa-coordinator"]
