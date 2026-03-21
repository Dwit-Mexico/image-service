#!/usr/bin/env bash
# Genera y aplica el Kubernetes Secret del image-service desde un .env local.
# Uso: ./scripts/gen-secrets.sh [--apply]
set -euo pipefail

ENV_FILE="${ENV_FILE:-.env}"
SECRET_NAME="image-service"

if [[ ! -f "$ENV_FILE" ]]; then
  echo "No se encontró $ENV_FILE"
  exit 1
fi

# Cargar variables
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

# Construir argumentos --from-literal
ARGS=()
while IFS='=' read -r key value; do
  [[ "$key" =~ ^#.*$ || -z "$key" ]] && continue
  ARGS+=("--from-literal=${key}=${value}")
done < "$ENV_FILE"

CMD="kubectl create secret generic $SECRET_NAME ${ARGS[*]} --dry-run=client -o yaml"

if [[ "${1:-}" == "--apply" ]]; then
  echo "Aplicando secret '$SECRET_NAME'..."
  eval "$CMD" | kubectl apply -f -
  echo "Restarting deployment..."
  kubectl rollout restart deployment/"$SECRET_NAME"
  kubectl rollout status deployment/"$SECRET_NAME" --timeout=60s
else
  echo "Preview (usa --apply para aplicar):"
  eval "$CMD"
fi
