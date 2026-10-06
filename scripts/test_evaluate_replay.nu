#!/usr/bin/env nu
# Exercise the frozen-data boundary and full CLI workflow; e.g. nu scripts/test_evaluate_replay.nu target/debug/btimeline.
use ./evaluate_replay.nu validate-manifest

def main [binary: path = 'target/debug/btimeline'] {
    let binary = ($binary | path expand)
    let directory = (mktemp -d)
    try {
        let rows = (['train' 'holdout'] | enumerate | each {|item|
            let code = $item.item
            let offset = ($item.index * 100)
            let events = ([1000 5000 9000] | each {|at| {timestamp: (1000 + $at + $offset), type: cast, sourceID: 10, abilityGameID: 46376, fight: 1}})
            let log = {report: {code: $code, revision: 1, startTime: 0, endTime: 21000,
                masterData: {lang: en, gameVersion: 1, logVersion: 76,
                    actors: [{id: 10, name: 'Synthetic Boss', gameID: 99001, type: NPC, subType: Boss}],
                    abilities: [{gameID: 46376, name: 'Repeat B528', type: '1'}]},
                fights: [{id: 1, name: 'Unknown Encounter', encounterID: 99199, difficulty: 123, startTime: 1000, endTime: 21000, inProgress: false, kill: true,
                    enemyNPCs: [{id: 10, gameID: 99001}], enemyPets: []}]}, events: $events,
                collection: {schemaVersion: 1, toolVersion: '0.1.0', collectedAtUnixMs: 1, requests: {}, complete: true, nextPageTimestamp: null, reportCode: $code,
                    fightID: 1, startTime: 1000, endTime: 21000, eventCount: 3, pageCount: 1, pageStartTimes: [1000]}}
            let path = ($directory | path join $'($code).json')
            $log | to json | save $path
            {file: $path, sha256: (open --raw $path | hash sha256), report: $code, fight: 1, encounter: 99199, difficulty: 123, candidate_group: $code, split: $code}
        })
        let manifest = ($directory | path join inputs.csv)
        $rows | to csv | save $manifest
        validate-manifest $manifest $binary | ignore
        let output = ($directory | path join evaluation)
        let evaluated = (^nu scripts/evaluate_replay.nu $manifest $output --binary $binary | complete)
        if $evaluated.exit_code != 0 { error make {msg: $evaluated.stderr} }
        let result = (open ($output | path join evaluation.json))
        if ($result.groups | length) != 2 or ($result.groups | any {|group| $group.status != 'passed' or $group.train.pulls != 1 or $group.holdout.pulls != 1 or $group.holdout.matches != 3}) {
            error make {msg: 'train/holdout replay failed'}
        }
        let generation = (open ($output | path join primary 99199-123 draft.report.json))
        if $generation.slots.0.sampleCount != 1 or $generation.input.report != 'train' {
            error make {msg: 'holdout leaked into generation'}
        }
        let bad = ($directory | path join bad.csv)
        for changed in [($rows | update 1.candidate_group 'train'), ($rows | update 1.encounter 9), ($rows | update 1.sha256 ('0' | fill -w 64 -c '0'))] {
            $changed | to csv | save --force $bad
            if (try { validate-manifest $bad $binary | ignore; true } catch { false }) {
                error make {msg: 'invalid manifest accepted'}
            }
        }
        let overwrite = (^nu scripts/evaluate_replay.nu $manifest $output --binary $binary | complete)
        if $overwrite.exit_code == 0 { error make {msg: 'existing evaluation overwritten'} }

        # Keep successful train-only runs unexecuted for holdout, e.g. a new group with one pull.
        let train_only = ($directory | path join train-only.csv)
        $rows | where split == 'train' | to csv | save $train_only
        let train_output = ($directory | path join train-only)
        let trained = (^nu scripts/evaluate_replay.nu $train_only $train_output --binary $binary | complete)
        if $trained.exit_code != 0 or ((open ($train_output | path join evaluation.json)).groups | any {|g| $g.status != 'holdoutNotExecuted' or not $g.train.passed}) {
            error make {msg: 'successful train-only evaluation failed'}
        }

        # Competing branches and a rogue transition reject P7 recovery; absent holdout must not hide failure.
        let failing_rows = ([[1000 10000] [5000 6000]] | enumerate | each {|item|
            let code = $'train-only-($item.index)'
            let log = (open $rows.0.file
                | update report.code $code
                | update collection.reportCode $code
                | update report.masterData.actors [{id: 10, name: 'Synthetic Boss', gameID: 99001, type: NPC, subType: Boss} {id: 11, name: 'Synthetic Boss', gameID: 99002, type: Player, subType: Player}]
                | update report.masterData.abilities ([46376 46377 46378 46379 46380 46381] | each {|id| {gameID: $id, name: $'Ability ($id)', type: '1'}})
                | update events (($item.item | enumerate | each {|event|
                    {timestamp: (1000 + $event.item), type: cast, sourceID: 10, abilityGameID: (46376 + $event.index), fight: 1}
                }) | append [
                    {timestamp: (1100 + $item.item.0), type: cast, sourceID: 10, abilityGameID: (46378 + $item.index), fight: 1}
                    {timestamp: (1200 + $item.item.0), type: cast, sourceID: 10, abilityGameID: (46380 + $item.index), fight: 1}
                    {timestamp: (1100 + $item.item.0), type: cast, sourceID: 11, abilityGameID: (46379 - $item.index), fight: 1}
                ] | append (if $item.index == 0 {
                    [{timestamp: (4000 + $item.item.0), type: cast, sourceID: 11, abilityGameID: 46377, fight: 1}]
                } else { [] }))
                | update collection.eventCount {|log| $log.events | length })
            let path = ($directory | path join $'($code).json')
            $log | to json | save $path
            $rows.0 | update file $path | update sha256 (open --raw $path | hash sha256)
                | update report $code | update candidate_group $code | update split train
        })
        let failing_manifest = ($directory | path join failing.csv)
        $failing_rows | to csv | save $failing_manifest
        let failing_output = ($directory | path join failing)
        let failed = (^nu scripts/evaluate_replay.nu $failing_manifest $failing_output --binary $binary | complete)
        let failures = (open ($failing_output | path join evaluation.json)).groups
        if $failed.exit_code == 0 or ($failures | length) != 2 or ($failures | any {|g| $g.status != 'failed' or $g.train.passed or $g.train.exitCode == 0 or $g.train.windowMisses != 2 or $g.holdout != null}) {
            error make {msg: 'train replay failure hidden without holdout'}
        }
    } catch {|error|
        rm -r $directory
        error make {msg: $error.msg}
    }
    rm -r $directory
}
