#!/bin/sh
# Reports GitHub Actions runner spend for this repository, computed from job durations.
#
#     scripts/ci-cost.sh [days]        # default: 30
#
# Uses run/job timestamps rather than the billing API: `/settings/billing/actions` needs the
# `user` OAuth scope, and `/actions/workflows/{id}/timing` returns `{}` while billing is blocked —
# exactly when you most want the number. Durations are always readable.
#
# GitHub bills whole minutes per job and rounds up, which this reproduces. Prices are the public
# per-minute rates for private repositories; macOS is ~10x Linux, which is why runner choice
# dominates the bill far more than job count does.
set -eu

days=${1:-30}
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
since=$(date -u -v-"${days}"d +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d "${days} days ago" +%Y-%m-%dT%H:%M:%SZ)

printf 'Repository: %s\nSince:      %s (%s days)\n\n' "$repo" "$since" "$days"

# Every API result is materialised and checked before anything is summed. `/bin/sh` has no
# `pipefail`, so piping `gh api` straight into `awk` would report awk's status only: an expired
# token, a rate limit, or a transient 5xx would silently produce partial totals — or a confident
# "No completed jobs in this window." — and exit 0, understating spend exactly when it matters.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
# Created up front: an empty window never enters the loop below, and `awk` would then fail with
# "cannot open file" instead of reaching its own "No completed jobs in this window." path.
: > "$work/jobs"

if ! gh api --paginate "repos/$repo/actions/runs?created=>$since" --jq '.workflow_runs[].id' \
	> "$work/runs"; then
	echo "error: could not list workflow runs for $repo" >&2
	exit 1
fi

while read -r run_id; do
	[ -n "$run_id" ] || continue
	# `filter=all` because the default, `latest`, returns only the most recent attempt — yet every
	# earlier attempt of a re-run job was billed too, so the default silently understates spend on
	# exactly the runs you re-ran because they failed. Paginated for runs with many jobs.
	if ! gh api --paginate "repos/$repo/actions/runs/$run_id/jobs?filter=all&per_page=100" --jq '
			.jobs[]
			| select(.started_at != null and .completed_at != null)
			| [ (.labels[0] // "unknown"), .name,
			    (((.completed_at | fromdate) - (.started_at | fromdate)) / 60) ]
			| @tsv' >> "$work/jobs"; then
		echo "error: could not read jobs for run $run_id; totals would be incomplete" >&2
		exit 1
	fi
done < "$work/runs"

awk -F'\t' '
	# Public per-minute rates for private repositories.
	BEGIN {
		rate["ubuntu-latest"] = 0.008; rate["ubuntu-22.04"] = 0.008; rate["ubuntu-24.04"] = 0.008
		rate["macos-15"]      = 0.062; rate["macos-14"]     = 0.062; rate["macos-latest"] = 0.062
		rate["windows-latest"] = 0.016
	}
	{
		runner = $1; job = $2; minutes = $3
		billed = int(minutes); if (minutes > billed) billed += 1   # GitHub rounds up
		r = (runner in rate) ? rate[runner] : 0.008
		job_min[job] += billed; job_cost[job] += billed * r; job_runner[job] = runner
		run_min[runner] += billed; run_cost[runner] += billed * r
		total_min += billed; total_cost += billed * r; runs++
	}
	END {
		# Reachable only when the API genuinely returned no jobs: every request above is
		# checked, so this can no longer mean "the token expired".
		if (runs == 0) { print "No completed jobs in this window."; exit }
		printf "%-34s %-16s %8s %10s\n", "JOB", "RUNNER", "MINUTES", "COST"
		# Insertion sort by cost, descending. `asorti` is a gawk extension and macOS ships the
		# original awk, so the ordering is done by hand to keep this script dependency-free.
		n = 0
		for (j in job_cost) { n++; ordered[n] = j }
		for (i = 2; i <= n; i++) {
			key = ordered[i]
			k = i - 1
			while (k >= 1 && job_cost[ordered[k]] < job_cost[key]) { ordered[k + 1] = ordered[k]; k-- }
			ordered[k + 1] = key
		}
		for (i = 1; i <= n; i++) {
			j = ordered[i]
			printf "%-34s %-16s %8d %9.2f$\n", j, job_runner[j], job_min[j], job_cost[j]
		}
		printf "\n%-34s %-16s %8s %10s\n", "RUNNER TOTAL", "", "MINUTES", "COST"
		for (r in run_min)
			printf "%-34s %-16s %8d %9.2f$\n", r, "", run_min[r], run_cost[r]
		printf "\n%-51s %8d %9.2f$\n", "TOTAL", total_min, total_cost
	}
' "$work/jobs"
