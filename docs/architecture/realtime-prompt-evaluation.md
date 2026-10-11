# Realtime prompt evaluation, 2026-10-12

After the context/memory scenario implementation merged in #142, fresh serial runs used
`mlx-community/gemma-4-e4b-it-4bit` through the configured compatible Realtime service at
temperature 0. The service accepts one active session. Personal configuration, history and
memory were untouched; evaluations used copied datasets and synthetic, bounded memory seeds.

OpenAI's [voice prompting guide](https://developers.openai.com/api/docs/guides/voice-prompting)
provided heuristics for short sections, source priority and examples. This is a Gemma backend;
OpenAI prompting guidance does not establish its model behavior.

## Full ordinary regression

The 19-recording development set and 10-recording previously used holdout were rerun without
memory. The holdout is now a reused regression set, not independent evidence of generalization.
Semantic scores below are human coding-agent judgments applied consistently within this round.
All four reports were finalized, and all failed the configured quality targets (WER <=10%,
semantic >=90, proper nouns >=95%).

| Prompt | Set | WER % | Proper nouns % | Semantic score |
| --- | --- | ---: | ---: | ---: |
| Merged default | 19 development | 25.95 | 81.82 | 82.11 |
| Already installed, pending #141 | 19 development | 22.70 | 90.91 | 82.37 |
| Merged default | 10 reused holdout | 31.90 | 66.67 | 71.50 |
| Already installed, pending #141 | 10 reused holdout | 27.59 | 75.00 | 73.50 |

The installed prompt was already configured before this round; its difference from the merged
default is not a new improvement delivered by this fix. PR #141 remains separate.

## Prompt screening

Shorter sections and text-first output were tested on all 19 development recordings.
Text-first improved WER to 23.78% but changed morning to afternoon. The compact prompt reached
29.19% WER and damaged the corrected meeting time. Neither was adopted.

Six fixed development recordings then screened context/memory instructions, positive examples,
field-schema instructions and combinations. The scenario covered known topics, learning,
independent support, irrelevant-history contamination and expiry. Manual vocabulary provided
names, not reference sentences or corrected dates/quantities. Candidate prompts still produced
wrong topic choices, superseded content and unsuitable terms. A full empty-output example caused
one voiced sample to return empty text; that run was incomplete and excluded from quality ranking.
No experimental prompt replaced the existing application prompt.

## Delivered candidate validation

The positive-example screen returned text `我要买两核和三核，嗯，三核吧，都是无统的。`
and candidate `3核38都是无统的`. The old client learned that candidate even though it did not
occur in the delivered text. The parser now requires substring support after NFKC normalization
and lowercasing. Valid text is retained, original JSON is retained for diagnosis, and the
unsupported candidate cannot enter learned memory. Redis matches fullwidth lowercase `ｒｅｄｉｓ`.

A fresh six-recording API run with the same prompt, audio and scenario produced identical raw
responses, input snapshots and text. The old final candidate list contained the fabricated
phrase; the new list was empty. The metadata gate now fails that segment. Both runs also failed
the Llama topic assertion; WER stayed 61.82% and noun accuracy stayed 100%. The fresh report was
semantically reviewed (70.83) and finalized with targets unmet. This ablation demonstrates a
memory-writing guard, not improved acoustic transcription or a superior prompt.

Substring support cannot establish acoustic truth, word boundaries, useful phrase types or
which spoken alternative was corrected. Wrong words already in the transcript may still become
history or candidates. Those quality failures remain visible rather than being claimed resolved.

Realtime policy advances to `realtime-context-memory-v2`. Safe v1 reports can still replay under
current parsing; reports whose stored snapshots depend on unsupported candidates fail validation.
Cross-policy comparisons reject before networking and require a fresh baseline. Ordinary schema-1
reports are unaffected. Tests cover candidate filtering, memory learning, metadata failure and
legacy-policy comparison rejection; local validation passed 241 tests (2 opt-in tests ignored),
formatting and Clippy with warnings denied.

Local ignored evidence: `.cache/2026-10-11/prompt-quality-round2/` contains prompts, scenarios,
raw reports, semantic reviews and `guard-ablation-summary.json`. Original recordings and personal
settings were preserved. Audio and detailed reports are not published with the code change.
