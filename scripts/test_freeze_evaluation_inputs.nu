#!/usr/bin/env nu
# Verify candidate grouping and invalid-input rejection. Example: nu scripts/test_freeze_evaluation_inputs.nu
use ./freeze_evaluation_inputs.nu *

def main [] {
    let directory = (mktemp -d)
    try {
        let paths = (0..6 | each {|index| $directory | path join $'($index).json'})
        for path in $paths { '{}' | save $path }
        let rows = (0..6 | each {|index|
            {file: ($paths | get $index), sha256: (open --raw ($paths | get $index) | hash sha256), identity: [$'report-($index)' $index], group: (if $index < 6 { [8888 101] } else { [9999 101] }), complete: true, manifest_count_matches: true, final_cursor: null, out_of_fight: 0, missing_cast_abilities: []}
        })
        let audit = {files: $rows, duplicate_pull_candidates: [{files: [$paths.0 $paths.1]}]}
        let frozen = (write-manifest $audit | from csv)
        if ($frozen | length) != 7 or $frozen.0.candidate_group != $frozen.1.candidate_group or $frozen.0.split != 'train' or $frozen.1.split != 'train' or $frozen.5.split != 'holdout' or $frozen.6.split != 'train' {
            error make {msg: 'candidate grouping or split failed'}
        }
        let bad = ($audit | update files ($rows | update 1.group [9999 101]))
        let accepted = (try { write-manifest $bad | ignore; true } catch { false })
        if $accepted { error make {msg: 'cross-group candidate accepted'} }
        # A stale audit must not approve bytes written after inspection.
        '{"changed":true}' | save --force $paths.0
        let changed_accepted = (try { write-manifest $audit | ignore; true } catch { false })
        if $changed_accepted { error make {msg: 'changed file accepted after audit'} }
    } catch {|error|
        rm -r $directory
        error make {msg: $error.msg}
    }
    rm -r $directory
}
