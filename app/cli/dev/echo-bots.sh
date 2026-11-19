#!/usr/bin/env bash
# Test bots on the DEV hub (https://dev.justyo.app), never the live one, for one test account (OWNER): the app's
# receive path, notifications, bursts, groups and long names can be tried from a phone.
#
# Echo bots answer OWNER's Yos after a fixed delay:
#   ECHO1S   a Yo back after 1 s          ECHOBURST  3 Yos, 0.5 s apart
#   ECHO3S   a Yo back after 3 s          ECHOPIC    a photo (skills/yogpt/replies/001.jpg)
#   ECHO10S  a Yo back after 10 s
# People bots (first names, some 15-16 characters long) answer OWNER's direct Yos after 1-5 s and OWNER's Yos in
# their groups after 0.5-8 s, sometimes 2 or 3 times. Every 3-6 minutes a random member Yos into a random group on
# its own, and every 3-7 minutes a long-name bot Yos OWNER directly, 1-3 times in a row. Groups (see GROUP_SPECS):
#   #FAMILIE, #FUSSBALL (7 members), #WG, #LANGENAMEN (long names only); #BOTS has the echo bots.
#
#   app/cli/dev/echo-bots.sh setup OWNER      register the bots (once) and send OWNER friend requests
#   app/cli/dev/echo-bots.sh group OWNER      create the groups with OWNER in them (once), the bots accept
#   app/cli/dev/echo-bots.sh start OWNER      the supervisor in the background (one `yo listen` per bot)
#   app/cli/dev/echo-bots.sh stop             stop it (also $DIR/stop.sh)
#
# Only OWNER's Yos are answered (direct or in a group), so the bots never answer each other. State, logs and a copy of
# the binary live in $YO_DEVBOTS (default ~/.local/share/yo-devbots), never in the repository. Build first:
# cargo build --release --locked -p yo-cli. The hub pins are read from the iOS app's Hub.swift.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/../../.." && pwd)
DIR=${YO_DEVBOTS:-$HOME/.local/share/yo-devbots}
ECHOS=(ECHO1S ECHO3S ECHO10S ECHOBURST ECHOPIC)
LONG=(MAXIMILIAN_MUELL ALEXANDRAWINTERS KONSTANTIN_BERG CHRISTOPHERJONES)
PEOPLE=(PETER TOM ANNA LUKAS SOPHIE MAX LENA_S JONAS BEN_W "${LONG[@]}")
BOTS=("${ECHOS[@]}" "${PEOPLE[@]}")
# NAME:MEMBER,MEMBER,... (OWNER is added to each; the first member creates it)
GROUP_SPECS=(
  "#BOTS:ECHO1S,ECHO3S,ECHO10S,ECHOBURST,ECHOPIC"
  "#FAMILIE:PETER,ANNA,TOM"
  "#FUSSBALL:TOM,LUKAS,MAX,JONAS,BEN_W,MAXIMILIAN_MUELL,KONSTANTIN_BERG"
  "#WG:SOPHIE,LENA_S"
  "#LANGENAMEN:MAXIMILIAN_MUELL,ALEXANDRAWINTERS,KONSTANTIN_BERG,CHRISTOPHERJONES"
)
HUB=https://dev.justyo.app
PHOTO=$REPO/skills/yogpt/replies/001.jpg

mkdir -p -m 700 "$DIR" "$DIR/bin"

pins() {
  # The dev hub's keys, trust roots and zk params, as the app pins them.
  python3 -I - "$REPO/app/ios/YoApp/Sources/YoApp/Shared/Hub.swift" <<'EOF'
import re, sys
s = open(sys.argv[1]).read()
def dev(name):
    block = s[s.index(f"static let {name}"):]
    block = block[:block.index("\n  ]")]
    rest = block[block.index('"dev.justyo.app"') + 16:]
    return re.findall(r'"([A-Za-z0-9_-]{20,})"', re.split(r'\n\s*"[a-z.]+":', rest)[0])
print(f"export YO_HUB_KEYS={','.join(dev('keys'))}")
print(f"export YO_TRUST_ROOTS={','.join(dev('trustRoots'))}")
print(f"export YO_ZK_PARAMS={dev('zkParams')[0]}")
EOF
}

env_for() { # BOT: the environment of one bot
  eval "$(pins)"
  export YO_HUB=$HUB YO_STATE=$DIR/$1 YO_BIN=$DIR/bin/yo
}

yo_as() { local bot=$1; shift; (env_for "$bot"; "$YO_BIN" "$@"); }

install_bin() { install -m 755 "$REPO/target/release/yo" "$DIR/bin/yo"; }

json() { python3 -I -c "import json, sys; v = json.load(sys.stdin); print($1)"; }

read -r -d '' BOT_PY <<'EOF' || true
import os, random, sys, threading, time
sys.path.insert(0, os.environ["SKILLS"])
import yocli
bot, owner, photo = sys.argv[1], sys.argv[2], sys.argv[3]
sys.argv[0] = bot  # the name yocli.log prints
echo = {"ECHO1S": ([1], False), "ECHO3S": ([3], False), "ECHO10S": ([10], False),
        "ECHOBURST": ([0.5, 1.0, 1.5], False), "ECHOPIC": ([1], True)}.get(bot)
def delays(group):
    if echo:
        return echo[0]
    if not group:
        return [random.uniform(1, 5)]
    first = random.uniform(0.5, 8)
    return [first + i * random.uniform(0.7, 2) for i in range(random.choice([1, 1, 1, 2, 3]))]
yocli.registered()
owner_id = None
limits = yocli.Buckets(rate=1.0, burst=5.0)
def answer(to):
    try:
        yocli.yo("send", to, *(("--photo", photo) if echo and echo[1] else ()))
        yocli.log("answered")
    except yocli.Failed as f:
        yocli.log(f"send failed: {yocli.reason(f)}")
for event in yocli.listen("--drain-media"):
    if event.get("type") == "ready":
        yocli.log("ready")
    if owner_id is None:
        try:
            owner_id = yocli.yo("whois", owner)["user"]
        except yocli.Failed as f:
            yocli.log(f"whois {owner}: {yocli.reason(f)}")
            continue
    if event.get("type") != "message" or event.get("from") != owner_id:
        continue
    if not limits.allow(owner_id, time.monotonic()):
        continue
    for delay in delays(event.get("group")):
        threading.Timer(delay, answer, (event["reply_to"],)).start()
EOF

case ${1:-} in
setup)
  owner=${2:?owner username}
  install_bin
  for bot in "${BOTS[@]}"; do
    if [[ ! -e $DIR/$bot/state.sqlite ]]; then
      mkdir -p -m 700 "$DIR/$bot"
      if ! yo_as "$bot" register "$bot"; then # a taken name (409): pick another in PEOPLE
        rm -rf "${DIR:?}/$bot"
        exit 1
      fi
    fi
    yo_as "$bot" add "$owner"
  done
  ;;
group)
  owner=${2:?owner username}
  touch "$DIR/groups"
  for spec in "${GROUP_SPECS[@]}"; do
    name=${spec%%:*}
    IFS=, read -r -a members <<<"${spec#*:}"
    grep -q "^$name " "$DIR/groups" && continue
    ids=()
    for m in "${members[@]:1}"; do ids+=("$(yo_as "$m" whoami | json 'v["user"]')"); done
    id=$(yo_as "${members[0]}" groups create "$name" "$owner" "${ids[@]}" | json 'v["group"]')
    echo "$name $id ${members[*]}" >>"$DIR/groups"
    for m in "${members[@]:1}"; do # the invitation fetched, then accepted
      yo_as "$m" sync --wait 5 --drain-media >/dev/null 2>&1 || true
      yo_as "$m" groups accept "$id" >/dev/null
    done
    echo "$name: ${members[*]}"
  done
  ;;
start)
  owner=${2:?owner username}
  install_bin
  "$0" stop >/dev/null 2>&1 || true
  cat >"$DIR/stop.sh" <<EOF
#!/usr/bin/env bash
exec "$REPO/app/cli/dev/echo-bots.sh" stop
EOF
  chmod 700 "$DIR/stop.sh"
  setsid nohup "$0" supervise "$owner" >>"$DIR/supervisor.log" 2>&1 </dev/null &
  echo "started (pid $!), logs in $DIR/*.log"
  ;;
supervise)
  owner=${2:?owner username}
  echo $$ >"$DIR/supervisor.pid"
  trap 'kill 0' TERM INT
  for bot in "${BOTS[@]}"; do
    [[ -e $DIR/$bot/state.sqlite ]] || continue
    (
      env_for "$bot"
      export SKILLS=$REPO/skills
      while :; do
        echo "$(date -Is) start"
        code=0
        nice -n 10 python3 -I -B -c "$BOT_PY" "$bot" "$owner" "$PHOTO" || code=$?
        echo "$(date -Is) exited $code"
        [[ $code == 3 || $code == 4 ]] && break # gone for good, or nothing registered
        sleep 5
      done
    ) >>"$DIR/$bot.log" 2>&1 &
  done
  ( # every 3-6 minutes a random member of a random people group Yos into it (sent through its listener)
    sleep 60
    while :; do
      line=$(grep -v '^#BOTS ' "$DIR/groups" 2>/dev/null | shuf -n1) || true
      if [[ -n $line ]]; then
        read -r name id members <<<"$line"
        bot=$(tr ' ' '\n' <<<"$members" | shuf -n1)
        yo_as "$bot" send "group:$id" >/dev/null && echo "$(date -Is) $bot -> $name" || echo "$(date -Is) $bot -> $name failed"
      fi
      sleep $((180 + RANDOM % 181))
    done
  ) >>"$DIR/ticker.log" 2>&1 &
  ( # every 3-7 minutes a long-name bot Yos OWNER directly, 1-3 times in a row
    sleep 120
    while :; do
      bot=$(printf '%s\n' "${LONG[@]}" | shuf -n1)
      for _ in $(seq $((1 + RANDOM % 3))); do
        yo_as "$bot" send "$owner" >/dev/null && echo "$(date -Is) $bot -> $owner" || echo "$(date -Is) $bot failed"
        sleep 1
      done
      sleep $((180 + RANDOM % 241))
    done
  ) >>"$DIR/ticker.log" 2>&1 &
  wait
  ;;
stop)
  if [[ -s $DIR/supervisor.pid ]] && kill -0 "$(cat "$DIR/supervisor.pid")" 2>/dev/null; then
    kill -TERM -- -"$(cat "$DIR/supervisor.pid")" && echo stopped
  else
    echo "not running"
  fi
  rm -f "$DIR/supervisor.pid"
  ;;
*)
  sed -n '2,21p' "$0"
  exit 2
  ;;
esac
