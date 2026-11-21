# DuckDB + Python image. Build context is the repo root:
#   docker build -f ref/Dockerfile -t ss:ref .
# Based on the organizers' DuckDB starter. No platform is pinned; build for linux/amd64.
FROM python:3.12-slim-bookworm

# The scorer runs the container as uid 1000; give that uid a name and a home.
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app

COPY ref/requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt

COPY ref/pipeline/ /app/pipeline/
COPY ref/run.sh /app/run.sh
RUN chmod +x /app/run.sh

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /app
CMD ["/app/run.sh"]
