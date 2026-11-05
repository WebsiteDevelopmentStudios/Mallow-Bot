FROM eclipse-temurin:21-jre-jammy

RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/mcxboxbroadcast

RUN curl -fL --retry 5 --retry-delay 2 \
    https://github.com/MCXboxBroadcast/Broadcaster/releases/latest/download/MCXboxBroadcastStandalone.jar \
    -o MCXboxBroadcastStandalone.jar

COPY entrypoint.sh /opt/mcxboxbroadcast/entrypoint.sh
COPY health_server.py /opt/mcxboxbroadcast/health_server.py
RUN chmod +x /opt/mcxboxbroadcast/entrypoint.sh

ENV MCXB_SERVER_ADDRESS=bliss1.atbp.fun
ENV MCXB_SERVER_PORT=19205
ENV PORT=10000

EXPOSE 10000
ENTRYPOINT ["/opt/mcxboxbroadcast/entrypoint.sh"]
