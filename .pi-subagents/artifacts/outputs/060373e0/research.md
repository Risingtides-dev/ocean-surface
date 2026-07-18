# Research: TikTok sound URL metadata

## Summary
The public TikTok sound URL is indexed as **“Ring trend” — Audreyy**. Confidence: **high (0.95)**, because a search result for a TikTok page displays the exact music label, and an independent TikTok sound index maps the exact numeric sound ID to the same title and author.

## Findings
1. **Exact ID/title/author match** — Tokchart’s indexed sound data links sound ID `7406721212087241477` to **Ring trend** by **Audreyy**. [Tokchart sound listing](https://tokchart.com/dashboard/sounds/7406721212087241477)
2. **TikTok-rendered usage confirms label** — A public TikTok video search result includes “Watch more videos with music **Ring trend - Audreyy**,” demonstrating TikTok’s indexed page metadata uses that exact label. [TikTok video result](https://www.tiktok.com/@fitfooddiary/video/7650744417459899670)
3. **Independent contextual corroboration** — Know Your Meme identifies a TikTok post using the sound as “Ring trend – Audreyy.” [Know Your Meme](https://knowyourmeme.com/memes/people/sydney-thomas)

## Sources
- Kept: [TikTok sound URL](https://www.tiktok.com/music/Ring-trend-7406721212087241477) — requested public source URL; direct page inspection was limited by TikTok rendering/access behavior.
- Kept: [Tokchart sound ID page](https://tokchart.com/dashboard/sounds/7406721212087241477) — exact numeric ID and title/author mapping.
- Kept: [TikTok indexed video result](https://www.tiktok.com/@fitfooddiary/video/7650744417459899670) — TikTok metadata explicitly shows “Ring trend - Audreyy.”
- Kept: [Know Your Meme](https://knowyourmeme.com/memes/people/sydney-thomas) — independent corroborating usage attribution.
- Dropped: unrelated generic “ring trend” SEO pages — no exact sound ID or Audreyy attribution.

## Gaps
The sound page itself could not be fully interactively inspected in this search environment. The conclusion is nevertheless well-supported by exact-ID third-party indexing plus TikTok-rendered metadata.

## Supervisor coordination
No coordination needed.

## Acceptance report
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete finding and severity: no issue found; exact public URL, exact sound ID, title, and author are documented in this file."
    }
  ],
  "changedFiles": [
    "/Users/smathdaddy-macbook/ocean-surface/.pi-subagents/artifacts/outputs/060373e0/research.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [],
  "validationOutput": [
    "Web searches returned exact match: sound ID 7406721212087241477 = Ring trend by Audreyy."
  ],
  "residualRisks": [
    "TikTok sound page could not be fully interactively rendered; confidence remains high based on corroborating indexed sources."
  ],
  "noStagedFiles": true,
  "diffSummary": "No project/source files modified; research artifact only.",
  "reviewFindings": [
    "no blockers"
  ],
  "manualNotes": "Finding: yes, indexed as Audreyy - Ring trend (display order on sources: Ring trend - Audreyy). Confidence 0.95."
}
```