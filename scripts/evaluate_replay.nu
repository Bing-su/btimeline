#!/usr/bin/env nu

# Freeze bytes and candidate splits before generating anything; e.g. nu scripts/evaluate_replay.nu docs/evaluation-inputs.csv /tmp/p6 --binary target/debug/btimeline.
export def validate-manifest [manifest: path, binary: path] {
    let rows = (open --raw $manifest | from csv | each {|row|
        $row | update file ($row.file | path expand)
            | update fight ($row.fight | into int)
            | update encounter ($row.encounter | into int)
            | update difficulty ($row.difficulty | into int)
    })
    if ($rows | is-empty) or (($rows | get file | uniq | length) != ($rows | length)) {
        error make {msg: 'empty manifest or duplicate file'}
    }
    for row in $rows {
        if $row.split not-in ['train' 'holdout'] or $row.sha256 !~ '^[0-9a-f]{64}$' {
            error make {msg: 'invalid split or SHA-256'}
        }
        if (open --raw $row.file | hash sha256) != $row.sha256 {
            error make {msg: $'input hash changed: ($row.file)'}
        }
    }
    for candidate in ($rows | group-by candidate_group | values) {
        if ($candidate | get split | uniq | length) != 1 or ($candidate | select encounter difficulty | uniq | length) != 1 {
            error make {msg: 'candidate crosses split or encounter/difficulty'}
        }
    }
    # Check the frozen identities against the collector boundary, including duplicate report/fight rejection.
    let inspection = (^$binary inspect-inputs ...($rows | get file) | complete)
    if $inspection.exit_code != 0 { error make {msg: $inspection.stderr} }
    let groups = ($inspection.stdout | from json)
    for group in $groups {
        for pull in $group.pulls {
            let row = ($rows | where file == $pull.file | first)
            if $row.report != $pull.report or $row.fight != $pull.fight or $row.encounter != $group.key.encounter or $row.difficulty != $group.key.difficulty {
                error make {msg: $'manifest identity mismatch: ($pull.file)'}
            }
        }
    }
    $rows
}

def summarize [report: record] {
    let summaries = ($report.pulls | get replay.summary)
    let errors = ($report.pulls | each {|pull| $pull.replay.rows | each {|row| $row.observations | get errorMs } | flatten } | flatten | each {|ms| $ms | math abs } | sort)
    mut result = {passed: $report.validation.replay, pulls: ($report.pulls | length), reports: ($report.pulls | get report | uniq | length)}
    for key in [activeSyncRows disabledSyncRows matches wrongMatches ambiguousMatches wrongJumps dependencyViolations windowMisses missing censored unsupported unverified] {
        $result = ($result | insert $key ($summaries | get $key | math sum))
    }
    let timing = if ($errors | is-empty) { {samples: 0, medianAbsMs: null, p95AbsMs: null, maxAbsMs: null} } else {
        let index = ((($errors | length) * 0.95 | math ceil | into int) - 1)
        {samples: ($errors | length), medianAbsMs: ($errors | math median), p95AbsMs: ($errors | get $index), maxAbsMs: ($errors | last)}
    }
    $result | insert timing $timing
}

def replay-split [binary: path, yaml: path, folder: path, output: path] {
    let run = (^$binary replay $yaml $folder -o $output | complete)
    if not ($output | path exists) { error make {msg: $'replay produced no report: ($run.stderr)'} }
    summarize (open $output) | insert exitCode $run.exit_code
}

def evaluate-group [rows: table, root: path, binary: path, mode: string] {
    let first = ($rows | first)
    let folder = ($root | path join $'($first.encounter)-($first.difficulty)')
    let train = ($rows | where split == 'train')
    let holdout = ($rows | where split == 'holdout')
    if ($train | is-empty) { return {group: ($first | select encounter difficulty), status: 'noTrainingInputs'} }
    mkdir ($folder | path join train)
    mkdir ($folder | path join holdout)
    # Hard links retain replay provenance without copying giant logs; never modify these staged inputs.
    for item in ($rows | enumerate) {
        let target = ($folder | path join $item.item.split $'($item.index).json')
        let linked = (^ln -- $item.item.file $target | complete)
        if $linked.exit_code != 0 { cp $item.item.file $target }
        if (open --raw $target | hash sha256) != $item.item.sha256 {
            error make {msg: $'staged input hash changed: ($target)'}
        }
    }
    let yaml = ($folder | path join draft.yaml)
    let generated = (^$binary generate ($folder | path join train) --mode $mode -o $yaml | complete)
    if $generated.exit_code != 0 { error make {msg: $generated.stderr} }
    let training = (replay-split $binary $yaml ($folder | path join train) ($folder | path join train.replay.json))
    let testing = if ($holdout | is-empty) { null } else {
        replay-split $binary $yaml ($folder | path join holdout) ($folder | path join holdout.replay.json)
    }
    # A missing holdout cannot hide training failure, e.g. active median syncs miss their corrected windows.
    {group: ($first | select encounter difficulty), status: (if not $training.passed { 'failed' } else if $testing == null { 'holdoutNotExecuted' } else if $testing.passed { 'passed' } else { 'failed' }), train: $training, holdout: $testing,
        timelineSha256: (open --raw $yaml | hash sha256), generationReportSha256: (open --raw ($folder | path join draft.report.json) | hash sha256),
        inputs: $rows}
}

def main [manifest: path, output: path, --binary: path = 'target/release/btimeline', --mode: string = 'raid'] {
    if $mode not-in ['raid' 'dungeon'] { error make {msg: 'mode must be raid or dungeon'} }
    if ($output | path exists) { error make {msg: 'evaluation output already exists'} }
    let binary = ($binary | path expand)
    let rows = (validate-manifest $manifest $binary)
    let output = ($output | path expand)
    mkdir $output
    let unique = ($rows | group-by candidate_group | values | where {|group| ($group | length) == 1} | flatten)
    mut results = []
    for evaluation in [{name: primary, rows: $rows}, {name: withoutDuplicateCandidates, rows: $unique}] {
        let root = ($output | path join $evaluation.name)
        mkdir $root
        for group in ($evaluation.rows | group-by {|row| $'($row.encounter)-($row.difficulty)'} | values) {
            let result = (try { evaluate-group $group $root $binary $mode } catch {|error|
                {group: ($group | first | select encounter difficulty), status: 'error', error: $error.msg}
            })
            $results = ($results | append ($result | insert evaluation $evaluation.name))
        }
    }
    let result = {toolVersion: (^$binary --version | str trim), binarySha256: (open --raw $binary | hash sha256), nushellVersion: (version | get version), manifestSha256: (open --raw $manifest | hash sha256), mode: $mode,
        inputs: $rows, groups: $results, cactbotParser: 'notExecuted', runtime: 'notExecuted',
        scope: 'Frozen regression inputs only; no general real-encounter performance claim.'}
    $result | to json --indent 2 | save ($output | path join evaluation.json)
    let header = "# P6 holdout 평가\n\n| 평가 | encounter | difficulty | 상태 | train pull | holdout pull | 오매칭 | jump 오류 | 의존성 위반 | window 미검출 | 미검증 |\n| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n"
    let lines = ($results | each {|r|
        $'| ($r.evaluation) | ($r.group.encounter) | ($r.group.difficulty) | ($r.status) | ($r.train?.pulls? | default 0) | ($r.holdout?.pulls? | default 0) | ($r.holdout?.wrongMatches? | default 0) | ($r.holdout?.wrongJumps? | default 0) | ($r.holdout?.dependencyViolations? | default 0) | ($r.holdout?.windowMisses? | default 0) | ($r.holdout?.unverified? | default 0) |'
    } | str join "\n")
    $header + $lines + "\n\n파일·행별 결과와 시간 오차는 각 그룹의 replay JSON/Markdown에 있습니다. holdout 없는 그룹과 실패·미지원은 통과로 간주하지 않습니다.\n" | save ($output | path join evaluation.md)
    if ($results | any {|r| $r.status in ['failed' 'error' 'noTrainingInputs']}) {
        error make {msg: $'evaluation failed; see ($output | path join evaluation.json)'}
    }
}
