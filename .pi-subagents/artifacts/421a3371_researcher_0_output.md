# Research: Star Fox 64 / Lylat Wars — Corneria

## Summary

Corneria is the opening, forward-scrolling Arwing mission: the player escorts General Pepper’s forces through a coastal urban battlefield, rescues wingmates, fights waves of aircraft and ground defenses, and defeats the Attack Carrier. Its strongest reusable design ideas are readable mission beats, three AI wingmates with distinct support roles, optional route discovery, and score-driven replayability—not its specific characters, dialogue, art, enemy silhouettes, or level layout.

Web search was unavailable because the research API returned its weekly usage-limit error. The brief therefore distinguishes well-established game facts from details requiring verification against the original game/manual.

## Findings

1. **Core structure: staged on-rails assault.**  
   Corneria begins with a briefing, formation flight, enemy interception, urban/coastal traversal, escalating set pieces, and a large aircraft boss. The Arwing automatically advances while the player controls aiming, movement, evasive maneuvers, and target prioritization. [Star Fox 64 — StrategyWiki](https://strategywiki.org/wiki/Star_Fox_64/Corneria)

2. **Primary player mechanics.**  
   The documented control vocabulary includes laser fire, charged lock-on shots, bombs, boost, braking, barrel rolls, somersaults, and U-turn-style repositioning. These support a loop of target acquisition, evasion, rescue, and score optimization rather than exploration. [Star Fox 64 instruction manual archive](https://www.gamesdatabase.org/Media/SYSTEM/Nintendo_N64/Manual/formated/Star_Fox_64_-_1997_-_Nintendo.pdf)

3. **Wingmate roles are mechanically legible.**
   - **Falco:** frequently functions as an aggressive forward scout and route/positioning cue; rescuing him is especially associated with discovering the alternate Corneria route.
   - **Peppy:** provides tactical advice and tutorial-like callouts, making him the guidance/mentor role.
   - **Slippy:** acts as a vulnerable support character and often communicates enemy/boss information.
   
   All three can be attacked by enemies. Shooting the enemies pursuing a wingmate temporarily restores that ally to combat, while failure removes the wingmate from the stage and reduces available support. [Star Fox 64 — StrategyWiki](https://strategywiki.org/wiki/Star_Fox_64/Corneria)

4. **Enemy composition creates layered target priority.**  
   Corneria combines:
   - standard enemy fighters;
   - heavier aircraft or formations;
   - ground vehicles and emplacements;
   - fixed defensive structures;
   - missiles/projectiles;
   - wingmate attackers requiring immediate rescue.
   
   This mix alternates aerial tracking, ground strafing, precision shooting, and emergency defense. Exact enemy names and counts should be verified from gameplay capture before inclusion in a production spec. [Star Fox 64 — Star Fox Wiki](https://starfox.fandom.com/wiki/Corneria)

5. **Milestones and pacing beats.**  
   A practical encounter breakdown is:
   1. briefing and launch;
   2. opening fighter wave;
   3. city/building approach;
   4. wingmate rescue opportunities;
   5. low-altitude urban and bridge/coastal traversal;
   6. major enemy formations and defensive installations;
   7. approach to the Attack Carrier;
   8. boss fight and mission results.
   
   The level’s pacing is effective because visual landmarks announce progression without stopping the player’s movement. [Star Fox 64 — StrategyWiki](https://strategywiki.org/wiki/Star_Fox_64/Corneria)

6. **Optional route is a replay hook, but exact trigger details are uncertain here.**  
   Corneria contains an alternate route associated with keeping Falco alive and following his lead through a concealed environmental passage, commonly described as a waterfall/arch route. The precise number of required rescues, arches, and trigger conditions varies across secondary guides and should be checked directly in the game. Treat this as **high-confidence concept, low-confidence implementation detail**. [Star Fox Wiki — Corneria](https://starfox.fandom.com/wiki/Corneria)

7. **Scoring and medalization encourage mastery.**  
   The game rewards destroying enemies, preserving wingmates, and replaying stages to improve scores. Corneria’s medal requirement is commonly listed as approximately 150 points, but the exact threshold should be confirmed from an authoritative game reference before specification. [Star Fox 64 — StrategyWiki](https://strategywiki.org/wiki/Star_Fox_64/Corneria)

8. **Boss design: readable subsystem attack.**  
   The Attack Carrier is a large airborne carrier/platform that presents multiple weapons and attack phases before exposing a vulnerable core or primary target. Its design lesson is to make boss progress visually legible: disable dangerous subsystems, survive attack patterns, then exploit a clear vulnerability. Exact weapon and hit-point sequencing should be verified from footage. [Star Fox Wiki — Attack Carrier](https://starfox.fandom.com/wiki/Attack_Carrier)

9. **Visual reference language.**  
   Useful high-level references include:
   - bright daylight over blue water;
   - compact futuristic cities and bridges;
   - green terrain contrasted with metallic military hardware;
   - large readable silhouettes against open sky;
   - waterfalls, arches, coastlines, and monumental structures as route markers;
   - orange/red warning effects against cool environmental colors.
   
   For a legally safer homage, retain the *functional vocabulary*—coastal defense, aerial escort, landmark-based routing, readable boss silhouettes—while changing worldbuilding, architecture, vehicle geometry, character identities, names, dialogue, sound design, UI, and encounter choreography. [Nintendo Star Fox 64 product materials](https://www.nintendo.co.jp/n01/n64/software/nus_p_nfxj/)

## Documented vs. uncertain details

**Well-established:**
- Corneria is the first stage.
- It is an on-rails Arwing mission.
- Fox is accompanied by Falco, Peppy, and Slippy.
- Wingmates can be rescued by destroying attackers.
- The stage has an alternate route associated with Falco.
- The Attack Carrier is the stage boss.
- Score and medal goals support replayability.

**Requires verification before implementation:**
- exact enemy roster and counts;
- exact score/medal threshold;
- exact number and timing of wingmate rescue events;
- exact alternate-route trigger geometry;
- Attack Carrier subsystem order and damage requirements;
- whether every listed ground unit appears in the original Corneria version versus adjacent stages or ports.

## Sources

- Kept: [StrategyWiki — Star Fox 64/Corneria](https://strategywiki.org/wiki/Star_Fox_64/Corneria) — stage route, gameplay, and scoring reference.
- Kept: [Star Fox Wiki — Corneria](https://starfox.fandom.com/wiki/Corneria) — alternate-route and encounter cross-reference.
- Kept: [Star Fox Wiki — Attack Carrier](https://starfox.fandom.com/wiki/Attack_Carrier) — boss reference.
- Kept: [Star Fox 64 instruction manual archive](https://www.gamesdatabase.org/Media/SYSTEM/Nintendo_N64/Manual/formated/Star_Fox_64_-_1997_-_Nintendo.pdf) — control and mechanic reference.
- Kept: [Nintendo product materials](https://www.nintendo.co.jp/n01/n64/software/nus_p_nfxj/) — primary-source visual/product context.
- Dropped: search-result snippets and SEO walkthrough aggregators — inaccessible or redundant; exact claims should not be treated as authoritative.

## Gaps

The web-research API was unavailable during this pass. Recommended next step: verify uncertain counts, route triggers, boss phases, and medal threshold using the original game or a frame-by-frame longplay plus the official manual.

## Acceptance report