# The Zombye dedicated server (docs/self-hosting.md).
#
#   docker build -t zombye-server .
#   docker run -d --name zombye -p 7777:7777 -v zombye-data:/data zombye-server
#
# then join it with the game: zombye --connect HOST
#
# The image holds the server binary and the game data, nothing else: no client, no art.
# /data is the server's folder: its config (server.ron, written with the defaults on the first
# start), its world (world.zsave) and whose survivor is whose (world.zsave.players).

FROM rust:1-bookworm AS build
WORKDIR /src
# The whole workspace's manifests are needed to resolve it; only the server is built (no Bevy).
# Some game data is compiled in, so it comes along.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY assets/data assets/data
RUN cargo build --release --locked -p zombye-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --user-group --no-create-home --home-dir /data zombye \
    && mkdir /data \
    && chown zombye:zombye /data
COPY --from=build /src/target/release/zombye-server /usr/local/bin/zombye-server
COPY assets/data /usr/share/zombye/data
COPY LICENSE-MIT LICENSE-APACHE NOTICE /usr/share/doc/zombye/
# Where the server finds the game data (items, buildings, loot...). Clients need the same data.
ENV ZOMBYE_DATA=/usr/share/zombye/data
USER zombye
WORKDIR /data
VOLUME /data
# Game clients (TCP). The admin MCP endpoint, if the config turns it on, is whatever port it says.
EXPOSE 7777
# `docker stop` sends SIGTERM: the server tells its players, saves the world and exits.
STOPSIGNAL SIGTERM
ENTRYPOINT ["zombye-server"]
CMD ["--config", "/data/server.ron"]
