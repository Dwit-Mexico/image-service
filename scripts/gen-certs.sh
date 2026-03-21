#!/usr/bin/env bash
# Genera la CA (una sola vez) y certificados por proyecto.
# Uso: ./scripts/gen-certs.sh <project-name>
#
# Ejemplo:
#   ./scripts/gen-certs.sh project-alpha    → project-alpha.crt + project-alpha.key
#   ./scripts/gen-certs.sh project-beta
set -euo pipefail

CERTS_DIR="${CERTS_DIR:-./certs}"
mkdir -p "$CERTS_DIR"

# ── CA (solo la primera vez) ──────────────────────────────────────────────────
if [[ ! -f "$CERTS_DIR/ca.key" ]]; then
  echo "Generando CA..."
  openssl genrsa -out "$CERTS_DIR/ca.key" 4096
  openssl req -x509 -new -key "$CERTS_DIR/ca.key" \
    -days 3650 -out "$CERTS_DIR/ca.crt" \
    -subj "/CN=ImageService-CA/O=TuEmpresa"
  echo "CA generada en $CERTS_DIR/ca.crt"
else
  echo "CA ya existe, reutilizando."
fi

# ── Servidor (solo la primera vez) ───────────────────────────────────────────
if [[ ! -f "$CERTS_DIR/server.key" ]]; then
  echo "Generando certificado servidor..."
  openssl genrsa -out "$CERTS_DIR/server.key" 2048
  openssl req -new -key "$CERTS_DIR/server.key" \
    -out "$CERTS_DIR/server.csr" \
    -subj "/CN=images-api.tuempresa.com"
  openssl x509 -req \
    -in "$CERTS_DIR/server.csr" \
    -CA "$CERTS_DIR/ca.crt" \
    -CAkey "$CERTS_DIR/ca.key" \
    -CAcreateserial \
    -out "$CERTS_DIR/server.crt" \
    -days 365
  echo "Cert servidor generado en $CERTS_DIR/server.crt"
fi

# ── Certificado cliente por proyecto ─────────────────────────────────────────
PROJECT="${1:-}"
if [[ -z "$PROJECT" ]]; then
  echo "Uso: $0 <project-name>"
  exit 1
fi

echo "Generando certificado para proyecto: $PROJECT"
openssl genrsa -out "$CERTS_DIR/${PROJECT}.key" 2048
openssl req -new \
  -key "$CERTS_DIR/${PROJECT}.key" \
  -out "$CERTS_DIR/${PROJECT}.csr" \
  -subj "/CN=${PROJECT}/O=TuEmpresa"
openssl x509 -req \
  -in "$CERTS_DIR/${PROJECT}.csr" \
  -CA "$CERTS_DIR/ca.crt" \
  -CAkey "$CERTS_DIR/ca.key" \
  -CAcreateserial \
  -out "$CERTS_DIR/${PROJECT}.crt" \
  -days 365

echo ""
echo "Entregar al proyecto '${PROJECT}':"
echo "  Cert: $CERTS_DIR/${PROJECT}.crt"
echo "  Key:  $CERTS_DIR/${PROJECT}.key"
echo ""
echo "Configurar en .env del image-service:"
echo "  PROJECT_$(echo "$PROJECT" | tr '[:lower:]-' '[:upper:]_')=${PROJECT}:sk_live_$(openssl rand -hex 20)"
