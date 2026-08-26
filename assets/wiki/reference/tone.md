---
id: tone
title: Tone and Art Direction
category: Reference
---

# Tone and Art Direction

Everything the game looks and sounds like is currently placeholder, and the
brief for replacing it lives in one line of TODO.md: *"late 1960s tech, with a
cutesy anime vibe. going for Wargame Red Dragon but anime."* That is enough to
recognise and not enough to build from, which is why the naming question in
TODO ("need to find a better name for *girls*") stayed stuck for so long — it
is a question about register, and the register was never written down.

This page is the place for that. Three things are now decided outright — the
**register**, **the word** and the **period** — and a fourth, **what the
shipped content already commits to**, turns out to be more than the roadmap
admits. What is left genuinely open is at the bottom.

## The register: serious in the field, lighthearted at home

**Decided.** The game runs at two registers and the split is by *place*, not by
intensity dial:

- **On the battlefield it is serious.** This is the Wargame half. The HUD stays
  austere and informational, the language is military, and a fight reads as a
  fight. Nothing here winks at the player.
- **At the academy it is lighthearted.** This is where the characters are
  people rather than crew stats, and where the visual-novel layer is going to
  live — support-conversation shaped, in the Fire Emblem tradition.

That split resolves several questions that were open a moment ago. The HUD
question is answered: cuteness is not sprayed across the interface, it is
concentrated in the academy screens, portraits and barks. And the "what are the
cadets *to* the fighting" question is answered as *in between* — they are
neither career soldiers nor schoolgirls play-acting, and the in-between is the
point, because it is what makes the two registers legible as two sides of the
same characters rather than tonal whiplash.

Consequences worth holding onto as things get built:

- The academy layer is not decoration. It is half the tone, so the menus and
  roster screens on TODO carry as much of the game's identity as the battle
  screen does — they should not be built as bare utility UI and prettied later.
- The VN layer wants the cadet-instance refactor that TODO already lists first
  under "Design Decisions to Lock Early". Support conversations need persistent
  per-cadet state (who has fought alongside whom, and how often) and that is the
  same mutable per-cadet object wounds and XP need. It is now load-bearing for
  two systems, not one.
- Crew death is still open, and this makes it sharper rather than softer: the
  more the academy half invests you in a specific cadet, the more a permadeath
  rule costs. Worth deciding before the VN work, not after.

## The word: cadet

**Decided, 2026-08-24.** They are **cadets**. The engine says so in its types
(`Cadet`, `CadetId`, `CadetStatus`), the UI says so on every screen, and the
prose here says so.

The objection worth answering is that *cadet* sounds like a low rank, and this
game runs the whole chain of command — a section leader and a company captain
are both on the roster. That objection mistakes what the word does. At an
academy, cadet is an **enrolment status, not a rank**: everyone enrolled is a
cadet, and the appointments — gunner, commander, platoon leader, captain of the
school team — are layered on top of it. Real academies do exactly this, with
cadet appointments running the full ladder. So the word spans the chain of
command precisely *because* it is not itself a rung on it, and "Cadet Krieger,
commanding 1st Company" is not a contradiction.

The cost is the one this page named when it ruled the institutional words out:
*cadet* is colder than the academy half wants. That is a real cost and it is
paid in the right place. The battlefield register wants the cold word — a fire
order says *cadet*, not a pet name — and the academy register does not use the
category noun at all. Nobody in a common room says "the cadets"; they say
Anka, or Rosa, or "the second-years". **The warmth belongs in the names, not in
the collective noun,** which is why picking a soft collective noun would have
put it in the one place it does not help.

Two things this does not settle, deliberately. It is not a claim about German
(*Kadett* is available and unglamorous in the same way, but no in-fiction
German is written yet). And it leaves room for a self-applied nickname to
appear later in the VN layer — a nickname is a thing characters coin, and it
sits on top of the institutional word rather than replacing it.

## What the shipped content already commits to

These are not aspirations; they are in `assets/mods/base/` today, and anything
new should either match them or change them on purpose.

**The hardware is German, by name and by model.** Every vehicle in the roster
is a real Wehrmacht designation: Luchs, Wiesel, Panther, Löwe, Marder, Hummel.
The guns are 37 / 75 / 88 mm plus a 105 mm howitzer. The player's academy is
Kuhlmann; the title is *Stahl*senshamädchen.

**The crews are not.** Of the forty-nine characters the base mod now ships,
twenty-nine carry German surnames and twenty do not — Japanese (Akiyama,
Tachibana), Italian (Ravenna, Marchetti), Russian (Belova, Orlov), Nordic
(Lindqvist, Halvorsen, Eide, Lindholm), French (Duval, Girard, Aubry), and
single instances of Spanish, Polish, Hungarian, Turkish and Balkan names. So
the mixed-nationality roster inside a nationally-themed school is the de facto
pattern — close to how Girls und Panzer handles it.

The mix is not even between the two academies, and that is deliberate
characterisation rather than an accident of name-picking: **Kuhlmann is a local
school** (eighteen of twenty-four are German) **and the Iron Valkyries recruit**
(eleven of twenty-five). It is the cheapest thing on this page to say
something with, and it costs no art.

**Factions are academies with military nicknames.** "Kuhlmann Academy" versus
"Iron Valkyries" — one institutional, one a unit moniker. Currently the only
two, and they only exist inside `frontier.json`.

**The palette is muted naturalism.** Terrain colours run olive and khaki
(`#7aa646` grass, `#b3a284` road, `#3f6d33` forest, `#8d7f70` town) against a
near-black `#171a21` background, with saturated primaries reserved for side
identity (`#4078c8` blue, `#cc463c` red). That is a deliberate-looking split —
the world is drab, the *sides* are bright — and it is the one part of the
current look that reads as design rather than placeholder.

**The UI is austere.** Monospace, left-aligned, no chrome, no ornament: it
reads far closer to Wargame than to anime. Nothing about it is cute yet.

## The period: set in the 1960s, fighting with what is to hand

**Decided.** The 1960s is *when the game takes place*, not a specification of
what everyone drives. Older equipment stays in service, so a Panther on the
field is not an anachronism to be corrected — it is a twenty-year-old tank that
somebody still has, still maintains, and still sends out.

This resolves what looked like a contradiction between the roster and the
brief, and it does so in the more interesting direction. It also happens to be
true to life: WWII armour soldiered on into the sixties and seventies in
second-line, reserve and training roles all over the world, which is exactly the
role a school's tanks would occupy.

What it buys, and what to hold onto:

- **The current roster is legitimate as-is.** Luchs, Wiesel, Panther, Löwe,
  Marder and Hummel are all fine. Nothing needs renaming.
- **The era is a spread, not a point.** New kit can arrive alongside the old —
  early ATGMs, stabilised guns, IR optics, the APC/IFV already on the roadmap —
  without displacing it. An academy fielding a 1943 medium against a 1965
  missile carrier is a normal matchup, not a special case.
- **This strengthens the argument for the ballistics rewrite.** A roster that
  spans twenty years of armour development is precisely where flat HP attrition
  breaks down: the whole point of a 1943 gun meeting 1960s armour is that it
  *cannot* get through, and the current `.max(1)` damage floor says it can, at
  six machine-gun bursts a round. Penetration modelling is what makes the
  period spread mean something instead of being a skin.
- **Equipment age is characterisation.** Who has the old kit and who has the new
  is a way to say something about an academy — its wealth, its politics, its
  self-image — before a word of dialogue. Worth using deliberately when the
  second faction gets fleshed out.

## Open questions

Nothing below can be answered from the code. Answers here unblock naming,
sprite briefs, and any UI work.

1. **Is the mixed-nationality roster inside a German-themed academy the
   intended pattern**, or an accident of ten placeholder names? If intended, do
   other academies get their own national kit the way GuP does?
2. ~~**What do we call the cadets?**~~ Answered above under *The word:
   cadet*. What is still open is the German, and whether the VN layer coins a
   nickname on top of it.
3. **Is hiring an artist near-term?** TODO says "worth contacting an actual
   artist and paying". If that is soon, the deliverable they need is a written
   brief with reference images, and this page should become that rather than a
   set of notes. Note the two-register split makes this a brief for *two* looks
   — field and academy — which is worth saying up front.
4. **How lethal is it allowed to look?** Girls und Panzer is famously
   non-lethal, and TODO already flags crew death as a core identity decision.
   The art answers a version of the same question: a destroyed tank that brews
   up reads very differently from one that pops a white flag.
