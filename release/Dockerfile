# Build context is a verified release artifact directory with webtest.bin.
FROM ubuntu:22.04@sha256:74b12b11ffa550fba1929ad4d8d16533c34f6612728469671afba1b03b66881c
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates fonts-liberation libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libcups2 libdbus-1-3 libdrm2 libgbm1 libglib2.0-0 libnspr4 libnss3 libpango-1.0-0 libx11-6 libxcb1 libxcomposite1 libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2 && rm -rf /var/lib/apt/lists/*
COPY --chmod=755 webtest.bin /usr/local/bin/webtest
RUN useradd --create-home --uid 10001 webtest && mkdir -p /workspace /artifacts /home/webtest/.cache && chown -R webtest:webtest /workspace /artifacts /home/webtest
USER 10001:10001
ENV HOME=/home/webtest
WORKDIR /workspace
RUN webtest browser install
ENTRYPOINT ["webtest"]
CMD ["--version", "--json"]
