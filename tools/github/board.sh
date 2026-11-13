#!/usr/bin/env bash
# board.sh: put issues on the deadcommons project boards and set their Status.
#
#   tools/github/board.sh add    REPO ISSUE            add the issue to its repo's board
#   tools/github/board.sh status REPO ISSUE STATUS     add it if needed, then set Status
#   tools/github/board.sh show   REPO ISSUE            the boards it is on and its Status there
#
# REPO is zombye or zombye-art (deadcommons/ may be put in front). STATUS is a
# Status option of the board, e.g. Todo, "In Progress", Done (case doesn't matter).
# --project N picks the board instead of the repo's default: zombye goes on
# project 3, zombye-art on project 1.
#
# Needs gh and a token that may write org projects: $GH_PROJECT_TOKEN, else the file
# ~/.config/zombye/gh-project-token (chmod 600). The token is never printed.
set -euo pipefail

OWNER=deadcommons
TOKEN_FILE="${ZOMBYE_BOARD_TOKEN_FILE:-$HOME/.config/zombye/gh-project-token}"

die() { echo "board.sh: $*" >&2; exit 1; }
usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

project=""
args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --project) [ $# -ge 2 ] || usage; project="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) args+=("$1"); shift ;;
  esac
done
[ ${#args[@]} -ge 3 ] || usage
cmd="${args[0]}"; repo="${args[1]#"$OWNER/"}"; issue="${args[2]}"
[[ "$issue" =~ ^[0-9]+$ ]] || die "issue must be a number, not '$issue'"

if [ -z "$project" ]; then
  case "$repo" in
    zombye) project=3 ;;
    zombye-art) project=1 ;;
    *) die "no default board for repo '$repo': pass --project N" ;;
  esac
fi

command -v gh >/dev/null || die "gh is not installed"
if [ -z "${GH_PROJECT_TOKEN:-}" ]; then
  [ -r "$TOKEN_FILE" ] || die "no token: put a deadcommons-caretaker token with org Projects read/write in $TOKEN_FILE (chmod 600), or set GH_PROJECT_TOKEN"
  GH_PROJECT_TOKEN="$(tr -d '[:space:]' < "$TOKEN_FILE")"
  [ -n "$GH_PROJECT_TOKEN" ] || die "$TOKEN_FILE is empty"
fi
export GH_TOKEN="$GH_PROJECT_TOKEN"
unset GITHUB_TOKEN

gql() { gh api graphql "$@"; }

# The board's id and its Status field, as "PROJECT_ID FIELD_ID" then one "OPTION_ID<TAB>NAME" a line.
board=$(gql -f owner="$OWNER" -F number="$project" -f query='
  query($owner: String!, $number: Int!) {
    organization(login: $owner) { projectV2(number: $number) {
      id
      field(name: "Status") { ... on ProjectV2SingleSelectField { id options { id name } } }
    } }
  }' --jq '.data.organization.projectV2 | "\(.id) \(.field.id)", (.field.options[] | "\(.id)\t\(.name)")') \
  || die "can't read board $project of $OWNER (does the token have org Projects access?)"
read -r project_id field_id <<<"$(head -n1 <<<"$board")"
[ -n "$project_id" ] && [ "$project_id" != null ] || die "board $project not found under $OWNER"

# The issue's id, and its item on this board if it is on it already.
found=$(gql -f owner="$OWNER" -f repo="$repo" -F number="$issue" -f query='
  query($owner: String!, $repo: String!, $number: Int!) {
    repository(owner: $owner, name: $repo) { issue(number: $number) {
      id title
      projectItems(first: 50) { nodes { id project { id number }
        fieldValueByName(name: "Status") { ... on ProjectV2ItemFieldSingleSelectValue { name } } } }
    } }
  }' --jq '.data.repository.issue | .id, .title, (.projectItems.nodes[] | "\(.id)\t\(.project.id)\t\(.project.number)\t\(.fieldValueByName.name // "-")")') \
  || die "can't read $OWNER/$repo#$issue (does the token have Issues read on $repo?)"
issue_id=$(sed -n 1p <<<"$found")
title=$(sed -n 2p <<<"$found")
[ -n "$issue_id" ] && [ "$issue_id" != null ] || die "$OWNER/$repo#$issue not found"
item_id=$(sed -n '3,$p' <<<"$found" | awk -F'\t' -v p="$project_id" '$2 == p { print $1; exit }')

add_item() {
  if [ -z "$item_id" ]; then
    item_id=$(gql -f project="$project_id" -f content="$issue_id" -f query='
      mutation($project: ID!, $content: ID!) {
        addProjectV2ItemById(input: { projectId: $project, contentId: $content }) { item { id } }
      }' --jq '.data.addProjectV2ItemById.item.id') || die "couldn't add #$issue to board $project"
    echo "added $repo#$issue ($title) to board $project"
  fi
}

case "$cmd" in
  show)
    sed -n '3,$p' <<<"$found" | awk -F'\t' -v r="$repo" -v n="$issue" \
      'NF { printf "%s#%s: board %s, Status %s\n", r, n, $3, $4; any = 1 } END { if (!any) printf "%s#%s is on no board\n", r, n }'
    ;;
  add)
    [ -z "$item_id" ] && add_item || echo "$repo#$issue is already on board $project"
    ;;
  status)
    [ ${#args[@]} -ge 4 ] || usage
    want="${args[3]}"
    option=$(sed -n '2,$p' <<<"$board" | awk -F'\t' -v w="$want" 'tolower($2) == tolower(w) { print $1; exit }')
    [ -n "$option" ] || die "board $project has no Status '$want'; it has: $(sed -n '2,$p' <<<"$board" | cut -f2 | paste -sd, -)"
    add_item
    gql -f project="$project_id" -f item="$item_id" -f field="$field_id" -f option="$option" -f query='
      mutation($project: ID!, $item: ID!, $field: ID!, $option: String!) {
        updateProjectV2ItemFieldValue(input: { projectId: $project, itemId: $item, fieldId: $field,
          value: { singleSelectOptionId: $option } }) { projectV2Item { id } }
      }' --jq '.data.updateProjectV2ItemFieldValue.projectV2Item.id' >/dev/null \
      || die "couldn't set Status of #$issue on board $project"
    echo "$repo#$issue ($title): Status $want on board $project"
    ;;
  *) usage ;;
esac
