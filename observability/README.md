# Gov Agent Observability Stack

This folder provides a local telemetry stack for `gov-agent`:

- Prometheus (metrics scrape + query)
- Grafana (dashboards)
- Tempo (trace storage)
- OpenTelemetry Collector (OTLP ingest + forward to Tempo)
- Starter Prometheus alert rules

## Run

```bash
docker compose up -d
```

## Endpoints

- Grafana: http://127.0.0.1:3000 (`admin` / `admin`)
- Prometheus: http://127.0.0.1:9090
- Tempo API: http://127.0.0.1:3200
- OTLP gRPC ingest: `127.0.0.1:4317`
- OTLP HTTP ingest: `127.0.0.1:4318`

## gov-agent config

Run `gov-agent` on the host with:

```bash
GOV_AGENT_METRICS_ENABLED=true
GOV_AGENT_METRICS_BIND=127.0.0.1:9464
GOV_AGENT_OTLP_ENDPOINT=http://127.0.0.1:4317
GOV_AGENT_OTLP_SERVICE_NAME=gov-agent-local
```

Prometheus scrapes `host.docker.internal:9464` from the container network.

## Included alerts

Prometheus loads these starter alerts from `prometheus/alerts.yml`:

- `GovAgentListenerStale`
- `GovAgentVoteSubmitFailures`
- `GovAgentRepeatedStageFailures`
