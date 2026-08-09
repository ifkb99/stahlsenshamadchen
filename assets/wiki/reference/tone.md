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
TODO ("need to find a better name for *girls*") has been stuck — it is a
question about register, and the register was never written down.

This page is the place for that. Two things are now decided outright — the
**register** and the **period** — and a third, **what the shipped content
already commits to**, turns out to be more than the roadmap admits. What is
left genuinely open is at the bottom.

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
girls *to* the fighting" question is answered as *in between* — they are
neither career soldiers nor schoolgirls play-acting, and the in-between is the
point, because it is what makes the two registers legible as two sides of the
same characters rather than tonal whiplash.

Consequences worth holding onto as things get built:

- The academy layer is not decoration. It is half the tone, so the menus and
  roster screens on TODO carry as much of the game's identity as the battle
  screen does — they should not be built as bare utility UI and prettied later.
- The VN layer wants the girl-instance refactor that TODO already lists first
  under "Design Decisions to Lock Early". Support conversations need persistent
  per-girl state (who has fought alongside whom, and how often) and that is the
  same mutable per-girl object wounds and XP need. It is now load-bearing for
  two systems, not one.
- Crew death is still open, and this makes it sharper rather than softer: the
  more the academy half invests you in a specific girl, the more a permadeath
  rule costs. Worth deciding before the VN work, not after.

## What the shipped content already commits to

These are not aspirations; they are in `assets/mods/base/` today, and anything
new should either match them or change them on purpose.

**The hardware is German, by name and by model.** Every vehicle in the roster
is a real Wehrmacht designation: Luchs, Wiesel, Panther, Löwe, Marder, Hummel.
The guns are 37 / 75 / 88 mm plus a 105 mm howitzer. The player's academy is
Kuhlmann; the title is *Stahl*senshamädchen.

**The crews are not.** Of ten characters, six are German (Weiss, Falkenrath,
Brandt, Voss, Steiner, Krieger, Müller), one Japanese (Juno Akiyama), one
Italian (Sofia Ravenna), one Russian (Nadja Orlov). So the mixed-nationality
roster inside a nationally-themed school is already the de facto pattern —
close to how Girls und Panzer handles it, and worth making explicit before more
characters are written.

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
2. **What do we call the girls?** The register is now settled as *in between*,
   which rules out both the purely institutional (*cadet*, *recruit* — too cold
   for the academy half) and the purely affectionate (too soft for the
   battlefield half). What is wanted is a word that can be said seriously over
   the radio and warmly in a common room. Still open: should it work in English
   and German both, and is it a rank, a self-applied nickname, or a coined
   term?
3. **Is hiring an artist near-term?** TODO says "worth contacting an actual
   artist and paying". If that is soon, the deliverable they need is a written
   brief with reference images, and this page should become that rather than a
   set of notes. Note the two-register split makes this a brief for *two* looks
   — field and academy — which is worth saying up front.
4. **How lethal is it allowed to look?** Girls und Panzer is famously
   non-lethal, and TODO already flags crew death as a core identity decision.
   The art answers a version of the same question: a destroyed tank that brews
   up reads very differently from one that pops a white flag.
