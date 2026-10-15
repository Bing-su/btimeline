#!/usr/bin/env nu

# Example: nu scripts/freeze_evaluation_inputs.nu audit.json > /tmp/inputs.csv
# Validate before emitting CSV so rejected audits never leave a partial manifest.
export def write-manifest [audit: record] {
    let rows = $audit.files
    if ($rows | is-empty) or (($rows | get file | uniq | length) != ($rows | length)) {
        error make {msg: 'empty audit or duplicate file path'}
    }
    for row in $rows {
        if $row.complete != true or $row.manifest_count_matches != true or $row.final_cursor != null or $row.out_of_fight != 0 or (not ($row.missing_cast_abilities | is-empty)) {
            error make {msg: $'incomplete or invalid audit: ($row.file)'}
        }
        # Bind audit findings to the exact bytes being frozen; reject changes after inspection.
        if (open --raw $row.file | hash sha256) != $row.sha256 {
            error make {msg: $'file changed since audit: ($row.file)'}
        }
    }

    mut memberships = []
    # One report is one independent observation source; linked uploads share its split too.
    mut report_groups = ($rows | each {|row| {report: $row.identity.0, group: $row.identity.0}} | uniq)
    for candidate in $audit.duplicate_pull_candidates {
        let members = ($candidate.files | sort)
        if ($members | length) < 2 or (($members | uniq | length) != ($members | length)) {
            error make {msg: 'invalid duplicate candidate'}
        }
        let selected = ($rows | where {|row| $row.file in $members})
        if ($selected | length) != ($members | length) {
            error make {msg: 'invalid duplicate candidate'}
        }
        if ($selected | get group | uniq | length) != 1 {
            error make {msg: 'duplicate candidate crosses input groups'}
        }
        let reports = ($selected | each {|row| $row.identity.0} | uniq)
        let connected = ($report_groups | where {|row| $row.report in $reports} | get group | uniq | sort)
        let representative = $connected.0
        $report_groups = ($report_groups | each {|row|
            if $row.group in $connected { $row | update group $representative } else { $row }
        })
        for path in $members {
            if ($memberships | any {|member| $member.file == $path}) {
                error make {msg: 'duplicate candidate overlaps another group'}
            }
            $memberships = ($memberships | append {file: $path, candidate_group: $members.0})
        }
    }

    let candidate_memberships = $memberships
    let components = $report_groups
    let groups = ($components | get group | uniq | sort)
    let records = ($rows | sort-by file | each {|row|
        let member = ($candidate_memberships | where file == $row.file)
        let candidate_group = if ($member | is-empty) { $row.file } else { $member.0.candidate_group }
        let component = ($components | where report == $row.identity.0 | first | get group)
        let index = ($groups | enumerate | where item == $component | first | get index)
        {file: $row.file, sha256: $row.sha256, report: $row.identity.0, fight: $row.identity.1, encounter: $row.group.0, difficulty: $row.group.1, candidate_group: $candidate_group, split: (if ($index mod 5) == 4 { 'holdout' } else { 'train' })}
    })
    $records | to csv --columns [file sha256 report fight encounter difficulty candidate_group split]
}

def main [audit_file: path] {
    print -n (write-manifest (open $audit_file))
}
