FROM eclipse-temurin:21-jre-jammy
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 python3-venv python3-pip ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir cryptography
ENV PATH="/opt/venv/bin:${PATH}"
WORKDIR /opt/mcxboxbroadcast
RUN curl -fL --retry 5 --retry-delay 2 https://github.com/MCXboxBroadcast/Broadcaster/releases/latest/download/MCXboxBroadcastStandalone.jar -o MCXboxBroadcastStandalone.jar
COPY mcxboxbroadcast-render/entrypoint.sh /opt/mcxboxbroadcast/entrypoint.sh
COPY mcxboxbroadcast-render/health_server.py /opt/mcxboxbroadcast/health_server.py
COPY mcxboxbroadcast-render/backup.py /opt/mcxboxbroadcast/backup.py
RUN chmod +x /opt/mcxboxbroadcast/entrypoint.sh
ENV MCXB_SERVER_ADDRESS=bliss1.atbp.fun
ENV MCXB_SERVER_PORT=19205
ENV PORT=10000
ENV MCXB_BACKUP_REPO=WebsiteDevelopmentStudios/mcxboxbroadcast-backups
ENV MCXB_BACKUP_INTERVAL=300
EXPOSE 10000
ENTRYPOINT ["/opt/mcxboxbroadcast/entrypoint.sh"]
