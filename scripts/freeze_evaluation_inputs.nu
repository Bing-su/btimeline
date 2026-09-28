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
        for path in $members {
            if ($memberships | any {|member| $member.file == $path}) {
                error make {msg: 'duplicate candidate overlaps another group'}
            }
            $memberships = ($memberships | append {file: $path, candidate_group: $members.0})
        }
    }

    let candidate_memberships = $memberships
    let grouped = ($rows | each {|row|
        let member = ($candidate_memberships | where file == $row.file)
        {file: $row.file, group: $row.group, candidate_group: (if ($member | is-empty) { $row.file } else { $member.0.candidate_group })}
    })
    let records = ($rows | sort-by file | each {|row|
        let candidate_group = ($grouped | where file == $row.file | first | get candidate_group)
        let groups = ($grouped | where group == $row.group | get candidate_group | uniq | sort)
        let index = ($groups | enumerate | where item == $candidate_group | first | get index)
        {file: $row.file, sha256: $row.sha256, report: $row.identity.0, fight: $row.identity.1, encounter: $row.group.0, difficulty: $row.group.1, candidate_group: $candidate_group, split: (if ($index mod 5) == 4 { 'holdout' } else { 'train' })}
    })
    $records | to csv --columns [file sha256 report fight encounter difficulty candidate_group split]
}

def main [audit_file: path] {
    print -n (write-manifest (open $audit_file))
}
