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

gh api --paginate "repos/$repo/actions/runs?created=>$since" --jq '.workflow_runs[].id' \
| while read -r run_id; do
	gh api "repos/$repo/actions/runs/$run_id/jobs" --jq '
		.jobs[]
		| select(.started_at != null and .completed_at != null)
		| [ (.labels[0] // "unknown"), .name,
		    (((.completed_at | fromdate) - (.started_at | fromdate)) / 60) ]
		| @tsv'
done \
| awk -F'\t' '
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
'
