#!/usr/bin/env sh
# Signs in a demo account, adds a made-up Garmin key and syncs the made-up data.
# The session token is public, so it only works in this demo.
set -eu
cd "$(dirname "$0")"
compose="docker compose -f docker-compose.yaml"
token=demo-session-token
url=http://localhost:8090

$compose exec -T db psql -q -U runmap runmap <<SQL
INSERT INTO users (oidc_issuer, oidc_subject, email, name)
VALUES ('https://demo.invalid', 'demo', 'alex@example.com', 'Alex Example')
ON CONFLICT DO NOTHING;
INSERT INTO sessions (token_hash, user_id, expires_at)
SELECT sha256('$token'), id, now() + interval '30 days' FROM users WHERE oidc_subject = 'demo'
ON CONFLICT DO NOTHING;
SQL

request() {
  curl --fail --silent --show-error --cookie "runmap_session=$token" --header "X-Requested-With: runmap" "$@"
}
request --request PUT --header "Content-Type: application/json" --data '{"api_key":"demo"}' "$url/api/me/garmin-key" >/dev/null
request --request POST "$url/api/sync" >/dev/null
while request "$url/api/sync" | grep -q '"state":"running"'; do sleep 2; done
request "$url/api/sync"
echo

# Official results for the made-up city runs, for the personal records.
request "$url/api/activities?q=City%20Run&limit=100" | python3 -c '
import json, sys
for activity in json.load(sys.stdin)["activities"]:
    meters = activity["distance_m"]
    # The race distance nearest to the run, at the run'"'"'s pace.
    race = min([5000, 10000, 21097.5, 42195], key=lambda distance: abs(distance - meters))
    print(activity["id"], round(activity["duration_s"] * race / meters), race)
' | while read -r id seconds distance; do
  request --request PUT --header "Content-Type: application/json" \
    --data "{\"distance_m\": $distance, \"time_s\": $seconds, \"event\": \"City Run\", \"results_url\": \"https://results.example/city-run\"}" \
    "$url/api/activities/$id/result"
done
echo "Open $url and set the cookie runmap_session=$token, or use the screenshots recipe."
