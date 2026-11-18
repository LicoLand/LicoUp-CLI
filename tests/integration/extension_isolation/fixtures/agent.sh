#!/bin/sh
# A synthetic extension process for the isolation suite.
#
# It is a real process on real pipes and it speaks the published C09 line
# protocol using shell builtins only — no command substitution, so it also runs
# under the restricted profile, which denies process creation. Modes that need a
# descendant (`descendant`, `noexit`, `stubborn`) or another binary
# (`writebomb`) are trusted-local fixtures and are documented as such.
#
# Modes: respond, delayed, no-cancel-method, crash, flood, descendant, noexit,
# stubborn, spin, writebomb, env, hang. The second argument is a marker the
# healthy modes report in their terminal body.
MODE="${1:-respond}"
MARKER="${2:-init}"

send() { printf '%s\n' "$1"; }

# The request id, extracted without a subshell: command substitution would fork,
# which a restricted profile denies.
take_id() {
  rest="${1#*\"id\":}"
  REQ_ID="${rest%%[!0-9]*}"
}

# One string field of the line, likewise fork-free.
take_field() {
  FIELD_VALUE=
  case "$1" in
    *"\"$2\":\""*)
      rest="${1#*\"$2\":\"}"
      FIELD_VALUE="${rest%%\"*}"
      ;;
  esac
}

if [ "$MODE" = hang ]; then
  # Never speaks the protocol at all: the host's initialize gets no answer.
  while :; do :; done
fi

CANCEL=unsupported
case "$MODE" in
  delayed | no-cancel-method | stubborn) CANCEL=acknowledged ;;
esac
# A 64-character run used by the flood mode.
X=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx

while IFS= read -r line; do
  case "$line" in
    *'"method":"extension.initialize"'*)
      take_id "$line"
      send '{"jsonrpc":"2.0","method":"extension.ready","params":{"profiles":["agent-execution"]}}'
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"protocol\":{\"major\":1,\"minimumMinor\":0},\"maxFrameBytes\":65536,\"profiles\":[\"agent-execution\"]}}"
      ;;
    *'"method":"agent.describe"'*)
      take_id "$line"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"id\":\"dev.example.agent.isolation.fixture\",\"instanceKind\":\"executable\",\"inputKinds\":[\"text\"],\"capabilities\":[\"dev.example.agent/fixture\"],\"interfaceVersion\":\"1.0.0\",\"usage\":\"unavailable\",\"cancel\":\"$CANCEL\",\"resume\":\"unsupported\"}}"
      ;;
    *'"method":"agent.execute"'*)
      take_id "$line"
      take_field "$line" invocationRef
      REF="$FIELD_VALUE"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"invocationRef\":\"$REF\",\"outcome\":\"accepted\"}}"
      terminal() {
        send "{\"jsonrpc\":\"2.0\",\"method\":\"agent.event\",\"params\":{\"invocationRef\":\"$REF\",\"sequence\":1,\"kind\":\"terminal\",\"body\":{\"outcome\":\"succeeded\",\"marker\":\"$MARKER\"}}}"
      }
      case "$MODE" in
        respond)
          terminal
          ;;
        delayed | no-cancel-method)
          # The work ends later than the cancel round trip, so a cancel really
          # is answered while the invocation is still in flight. The background
          # subshell needs a fork; this is a trusted-local fixture.
          ( sleep 2; terminal ) &
          ;;
        crash)
          sleep 1
          kill -9 $$
          ;;
        flood)
          # Unbounded output: lines far past any negotiated frame bound.
          while :; do
            i=0
            while [ $i -lt 256 ]; do
              printf '%s' "$X$X$X$X$X$X"
              i=$((i + 1))
            done
            printf '\n'
          done
          ;;
        descendant)
          # A background child that inherits the stdout pipe and never exits.
          # Trusted-local fixture: the fork inside the subshell is expected.
          ( while :; do :; done ) &
          printf '%s\n' "$!" >"./grandchild.pid"
          terminal
          ;;
        noexit)
          terminal
          # Refuses to exit even when stdin closes: only teardown ends it.
          while :; do sleep 30; done
          ;;
        stubborn)
          # Admits the work, never ends it and never exits on its own.
          while :; do sleep 30; done
          ;;
        spin)
          while :; do :; done
          ;;
        writebomb)
          dd if=/dev/zero of="./bomb" bs=4096 count=4096 2>/dev/null
          terminal
          ;;
        env)
          set >"./env.txt"
          terminal
          ;;
        *)
          terminal
          ;;
      esac
      ;;
    *'"method":"agent.cancel"'*)
      take_id "$line"
      take_field "$line" invocationRef
      if [ "$MODE" = no-cancel-method ]; then
        # An extension that does not implement cancellation says so; it never
        # claims the work stopped.
        send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"error\":{\"code\":-32601,\"message\":\"unsupported_method\"}}"
      else
        send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"invocationRef\":\"$FIELD_VALUE\",\"outcome\":\"$CANCEL\"}}"
      fi
      ;;
    *'"method":"agent.observe"'* | *'"method":"agent.resume"'*)
      take_id "$line"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"error\":{\"code\":-32601,\"message\":\"unsupported_method\"}}"
      ;;
    *'"method":"agent.history"'* | *'"method":"agent.models"'* | *'"method":"agent.reconcile"'*)
      take_id "$line"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"error\":{\"code\":-32601,\"message\":\"unsupported_method\"}}"
      ;;
    *'"method":"extension.shutdown"'*)
      case "$MODE" in
        stubborn | noexit)
          : # refuses to exit: the host has to tear the process group down
          ;;
        *)
          take_id "$line"
          send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"outcome\":\"stopped\"}}"
          exit 0
          ;;
      esac
      ;;
    *)
      : # notifications and unknown traffic are ignored
      ;;
  esac
done
