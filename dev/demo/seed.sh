#!/usr/bin/env sh
# Signs in a demo account, adds a made-up Garmin key and syncs the made-up data.
# The session token is public, so it only works in this demo.
set -eu
cd "$(dirname "$0")"
compose="docker compose -f docker-compose.yaml"
token=demo-session-token
url=http://localhost:8090

$compose exec -T db psql -q -U firn firn <<SQL
INSERT INTO users (oidc_issuer, oidc_subject, email, name)
VALUES ('https://demo.invalid', 'demo', 'alex@example.com', 'Alex Example')
ON CONFLICT DO NOTHING;
INSERT INTO sessions (token_hash, user_id, expires_at)
SELECT sha256('$token'), id, now() + interval '30 days' FROM users WHERE oidc_subject = 'demo'
ON CONFLICT DO NOTHING;
SQL

request() {
  curl --fail --silent --show-error --cookie "firn_session=$token" --header "X-Requested-With: firn" "$@"
}
request --request PUT --header "Content-Type: application/json" --data '{"api_key":"demo"}' "$url/api/me/garmin-key" >/dev/null
request --request POST "$url/api/sync" >/dev/null
while request "$url/api/sync" | grep -q '"state":"running"'; do sleep 2; done
request "$url/api/sync"
echo

# Official results for the made-up marathons, for the personal records. The
# time of a marathon is its recorded time.
request "$url/api/activities?q=Marathon&limit=100" | python3 -c '
import json, sys
for activity in json.load(sys.stdin)["activities"]:
    result = {
        "distance_m": 42195,
        "time_s": round(activity["duration_s"]),
        "event": activity["name"],
        "results_url": "https://results.example/marathon",
        "target_time_s": round(activity["duration_s"]) - 300,
    }
    print(activity["id"], json.dumps(result), sep="\t")
' | while IFS="$(printf '\t')" read -r id result; do
  request --request PUT --header "Content-Type: application/json" --data "$result" "$url/api/activities/$id/result"
done
echo "Open $url and set the cookie firn_session=$token, or use the screenshots recipe."
