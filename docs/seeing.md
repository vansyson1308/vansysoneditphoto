# Seeing: observation views, chance frames and reproducible variants

> **Status:** first release · **Scope:** engine commands `seeing.*`, `document.transaction`, and the MCP tools that use them · **Vietnamese user guide:** [`docs/vi/art-of-seeing-va-mcp.md`](vi/art-of-seeing-va-mcp.md)

MagicPic's seeing tools help a person (or an agent working for one) *look* at a picture
before and while editing it, and try several ways of showing it without ever touching the
original. They came out of reading a 15-page Vietnamese excerpt of Freeman Patterson's
*Photography and the Art of Seeing*. The excerpt is not in this repository and nothing is
quoted from it; the mapping from its ideas to these features is in the Vietnamese guide.

Three rules shaped every command here:

1. **Nothing scores a picture.** There is no "quality" number and no auto-fix. Blur, grain,
   darkness and empty space are choices until the person says otherwise.
2. **Looking changes nothing.** A view is returned as an image. It is never written into the
   document, so it cannot reach an export by accident.
3. **Exploring happens on a copy, and can be repeated.** A variant is a new document made
   from a recipe; the recipe is stored with it and gives the same pixels again.

## Views (`seeing.view`, MCP `render_preview {view, region}`)

A view is a reduced render of a document (at most 2048 px on its longer side), or of one
rectangle of it, shown as it is or through one of five transforms
(`crates/algo/src/seeing.rs`):

| View | What it shows | How |
|---|---|---|
| `normal` | The picture as it is | The same reduced composite as the plain preview |
| `luminance` | Tones only | Each pixel as the grey of its luminosity (the Luminosity blend mode's definition) |
| `masses` | Colour areas without detail | A strong blur, 1–5 % of the long side by `strength`, on premultiplied colour |
| `shapes` | Figure against empty space | A light blur, then `levels` (2–8, default 3) equal bands of the picture's own 2nd–98th percentile tonal range |
| `lines` | Outlines without surface texture | A light blur, then Sobel edge strength relative to the picture's own strong edges (99.5th percentile); only what stands out against them, and never more than the strongest fifth of the picture, is drawn, dark on opaque white |
| `color` | Colour without tone | Every pixel moved to luminosity 0.5, hue and saturation kept (clipped towards grey where out of gamut) |

`seeing.view` is a query: no history state, no revision change, no journal entry, and a
floating selection stays floating. It returns
`{mime, base64, bytes, width, height, view, description, analysisOnly, region, document}`,
where `document` is `{id, name, width, height, revision}`. A `region` is rendered at its own
size, reduced only when it is larger than `maxSide`. Unknown parameters are errors.
`format: "jpeg"` gives a much smaller image (transparency on white) for clients with small
tool-result limits. `document` names a document by index without making it active.

In the app, **View › Seeing** opens a view as a separate document ("photo.jpg (lines study)")
beside the picture: `seeing.study.luminance`, `.masses`, `.shapes`, `.lines`, `.color`, or
`seeing.study {view, strength, levels, maxSide}`. A study is an 8-bit RGB picture of the
view; the source document is not changed.

Limits, stated plainly:

- Views are computed on the reduced composite, in the document's own RGB numbers; a linear
  composite (a document with a linear profile, such as an opened EXR) is given the sRGB curve first, exactly when the canvas
  does, so it looks as it does on screen. The image carries no profile: a wide-gamut document
  looks a little flatter in a view than in the app. Views are aids for comparing areas of a
  picture, not colorimetric measurements. Documents that are not RGB are viewed through the
  compositor's RGB rendering.
- `lines` scales to each picture's own strong edges, so two pictures' line views are not
  comparable in strength; where edges are faint next to the picture's strongest (a figure's
  shadow side) the outline stays open. `shapes` bands follow the picture's own tonal range, so
  a faint gradient inside an otherwise even area becomes bands, and a thin middle-tone rim
  follows every boundary between the darkest and lightest band.
- A view cannot recover what the photograph does not hold: another viewpoint, different
  light, or detail lost to clipping.

## Frames (`seeing.frames`)

`seeing.frames {seed, count, minFraction, maxFraction, aspect}` returns `count` rectangles
inside the active document, a pure function of the seed (SplitMix64). They are places to look
at (`seeing.view {region}`) or to crop a variant to. The point of the seed is that the choice
is not the viewer's habit, and that a frame someone liked can be found again.

## Variants and recipes (`seeing.variant`, `seeing.experiments`, `seeing.recipe`)

A **recipe** is JSON:

```json
{"magicpicSeeing": 1, "label": "exposure +2 EV", "kind": "overexpose", "rule": "expose correctly",
 "seed": 7, "frame": null,
 "steps": [{"id": "layer.newAdjustmentLayer.exposure", "params": {"exposure": 2, "name": "Seeing: +2 EV"}}],
 "source": {"name": "photo.jpg", "width": 6000, "height": 4000}}
```

`seeing.variant {recipe}` duplicates the active document, crops the copy to `frame` (pixels
outside are kept, so the crop can be changed later) and runs `steps`, which are ordinary
engine commands. It is all or nothing: if a step fails, or turns out to have changed, opened
or closed any other document, the whole session is put back as it was (see
[Transactions](#transactions-documenttransaction)): the copy is gone, the source is active
again, and the error names the step. There are at most 32 steps, and the same kinds of
command a transaction refuses are refused here.

The recipe is stored in the variant as a note (author `MagicPic Seeing`), so it is saved
with the document (tested for `.pcraft`; notes are also part of what the PSD writer saves)
and shows in Window › Notes. `seeing.recipe` reads it back. A variant starts its own record:
the source's recipe and reactions are not copied into it. Replaying a stored recipe on the
same source gives the same pixels, tested exactly at 8, 16 and 32-bit for every generated
experiment; a hand-written recipe repeats too as long as its steps do not depend on the
current tool colours, brush settings or on layer ids (a copy's layers get new ids).

A recipe read from a file is data someone else may have written. Replaying it runs its
commands on a copy, under the same refusals and the same file-access gate as any other
automation command, but look at the steps before replaying a recipe you did not make.

`seeing.experiments {seed, count, kinds}` proposes recipes that each break one rule on
purpose. It changes nothing until one is passed to `seeing.variant`
(`seeing.variant {kind, seed}` does both):

| Kind | Rule it breaks | Recipe |
|---|---|---|
| `overexpose`, `underexpose` | Expose correctly | An Exposure adjustment layer, ±1 to ±3 EV |
| `shake`, `pan` | Hold the camera steady | Stamp Visible → smart object → Motion Blur at 90° / 0°, 2–6 % of the long side |
| `spin`, `zoom` | Hold the camera steady | Stamp Visible → smart object → Radial Blur (spin / zoom) |
| `defocus` | Keep the subject sharp | Stamp Visible → smart object → Gaussian Blur, 0.4–2 % of the long side |
| `chance` | Choose by habit | A seeded frame plus one of the above, sized to the frame |

Every experiment stays editable: exposure is an adjustment layer, blurs are smart filters on
a stamp with the original layers still underneath. **View › Seeing › New Chance Variant**
runs `chance` with a seed taken from the document's state; the seed is in the stored recipe.

`seeing.note {reaction, reason}` records the viewer's own reaction to a variant as a note in
it (one history step). `seeing.recipe` returns the reactions with the recipe. The text is the
person's; nothing interprets it.

## Transactions (`document.transaction`)

`document.transaction {steps, expectRevision, expectDocument}` runs up to 256 commands on the
active document as one all-or-nothing edit.

- **Guard.** With `expectDocument` (the document's `id`) and `expectRevision` nothing runs
  unless the active document is that document at that revision. Both come from
  `document.inspect` / `session.list`. The id matters as much as the revision: every freshly
  opened document starts at the same revision, and in the app the person may have switched
  tabs since the caller looked.
- **Rollback.** Before the first step the state of every open document is noted
  (`Checkpoint`, cheap: documents and their histories are shared, not copied). After each
  step the session is checked: only the transaction's own document may have changed, and no
  document may have appeared, gone or become active. If a step fails that check or returns
  an error, every document, its history, snapshots, layer selection and floating selection
  are put back, documents the step opened are closed, the journal forgets the steps (so Last
  Filter, Transform Again and an action being recorded do not replay them) and the tool
  colours are re-synced. A restored document's revision moves forward, never back.
- **Refused up front** (nothing runs): commands that open, save, close, duplicate or switch
  documents or read and write files (`file.*`, `window.*`, `document.*`, exports and
  imports), and that change session-wide settings or libraries (preferences, presets,
  plug-ins, purge). Stepping through history (`edit.undo`, `edit.redo`,
  `history.stepBackward`, `history.selectState`, …) is allowed only in a transaction made of
  nothing else: "undo twice, if nobody edited since I looked" is the guarded way back, but a
  step that edits and a step that takes edits back do not make one edit.
- **Not rolled back:** the clipboard, tool and brush settings a step changed
  (`tools.setColors`), and anything outside the session.

Each step still records its own history state, so Undo steps back through a successful
transaction one command at a time. Steps run to the end, without progress reports or
cancellation, and a transaction does not start while a background job holds the document.

Because the whole transaction is one engine command, nothing else can touch the document
between its steps, in the headless server and in the desktop app alike. MCP exposes it as
`command_batch {atomic: true, expect_revision, expect_document}` and
`command_run {expect_revision, expect_document}`.

Known limit in the app: steps go straight to the engine, not through the shell's handling
of an open Free Transform or Crop box. Finish or cancel such a box before a transaction runs.

## What is deliberately not here

- No subject, face or sky detection is used by these commands, and no picture is sent
  anywhere. Deciding *what* to try is left to the person or to the agent they are talking to.
- No "quality" metric, no ranking of variants, no automatic choice between them.
- No canvas view mode yet: in the app a view opens as a study document. A live toggle on the
  canvas is tracked as follow-up work.
