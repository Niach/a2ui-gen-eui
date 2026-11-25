FROM rust:1.88-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends git python3 python3-venv python3-dev
WORKDIR /app
COPY requirements.txt vendor.py ./
RUN python3 -m venv /venv && /venv/bin/pip install --no-cache-dir -r requirements.txt && python3 vendor.py

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 user
WORKDIR /app
COPY --from=build /venv /venv
COPY --from=build /app/vendor ./vendor
COPY app.py ./
COPY static ./static
USER user
ENV HOST=0.0.0.0 PORT=7860
EXPOSE 7860
CMD ["/venv/bin/python", "app.py"]
