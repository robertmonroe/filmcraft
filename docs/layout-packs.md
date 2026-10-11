# Layout packs

A **layout** frames the footage of a scene: the speaker in a rounded window on a background, a
screen recording beside the speaker, a name plate over the picture, a title or chapter card. A
**layout pack** is a set of layouts that share one look (colours and fonts), so a whole video
keeps one style and can be restyled in one step. When one layout follows another, a **smart
transition** carries what they share from its old place to its new one.

Nothing is baked into the footage. A layout is a graphic clip on its own track above the
footage; its **media windows** show the tracks under it. The footage stays ordinary clips that
are trimmed, cut, colour corrected and edited by transcript as before, and the layout follows.

Layout packs are FilmCraft's own, open format (`.fcpack`, JSON, documented below). FilmCraft
does not read other applications' layout or template packages.

## Media windows

A graphic clip's layers are text, shapes and **media** layers ([graphics.md](graphics.md)). A
media layer is a box (size, corner radius) that shows video:

| Property (parameter id) | Meaning |
|---|---|
| `source` | *All Tracks Below*: every video track under the clip, composited as the sequence shows them. *Track Below n* (1–8): the clip on the n-th track under this one, alone |
| `fit` | *Fill* covers the box and crops what does not fit; *Fit* shows the whole picture (the layer's fill shows around it); *Stretch* ignores the aspect ratio |
| `focus` | The point of the picture kept in view when it is cropped, in percent of its width and height (50, 50 = the centre) |
| `zoom` | Extra magnification inside the box, in percent |
| `role` | What the window is for: `camera`, `screen`, `media` or empty. Used to pick its source when a layout is applied and to match windows across a smart transition |
| `size`, `corner_radius` | The box. A corner radius of half the size makes a circle |

All of them except `source`, `fit` and `role` take keyframes, so a window can push in on its
picture or travel across the frame. A media layer also has the appearance and transform
properties of a shape: its **fill** is what shows where there is no picture (an empty track, the
bars around a fitted frame), **strokes** make a border, and the **shadow**, opacity, position,
scale and rotation work as on any layer. Responsive pins resize a media layer like a shape.

*Track Below n* shows the clip's **own picture**: decoded, with its effects, masks and opacity,
before Motion places it in the sequence frame. The window then frames the footage whatever its
size and shape: 16:9 footage fills a window in a 9:16 sequence instead of arriving letterboxed.
While a transition runs on that track, the track's frame is shown instead. *All Tracks Below*
always shows the frame as the sequence composites it.

A window can look at a track that itself holds a layout (a picture in a picture in a picture);
this is followed two levels deep.

Frames with media windows are composited on the CPU (the same code as export), so what the
Program monitor shows is exact. When a layout's background is an opaque rectangle covering the
frame, the tracks it hides are not drawn a second time under it.

| Command | Params |
|---|---|
| `graphics.newMedia` | `source` (`"all"` or 1–8), `position` (the window's centre), `size`, `cornerRadius`, `fit`, `focus`, `zoom`, `role`, `name`, `clip` (add the window to this graphic), `seconds`, `track`, `time` |
| `graphics.set` | as for any layer: `props` `{source, fit, focus, zoom, role, size, cornerRadius, …}`; `source` and `fit` take names (`"track below 2"`, `"fit"`) or indices |
| `graphics.list` | each media layer reports `kind: "media"` and a `media` object |

## Packs and themes

A pack's **theme** is a set of named colours and fonts. Each layout binds layer parameters to
theme entries (the background's fill to `color.background`, a heading's font to `font.heading`),
so every layout of the pack takes the same look, and changing the theme restyles them all.

Each layout carries one **design** per frame shape it was drawn for. Placing a layout uses the
design nearest the sequence's aspect ratio, scaled to the frame; layers pinned to the frame
follow its edges. One pack therefore serves 16:9, 9:16, 1:1 and 4:5 deliveries.

The built-in packs are original designs made for FilmCraft in code, with the bundled fonts:

| Pack | Look |
|---|---|
| Studio | Dark and quiet, one warm accent |
| Paper | Light and editorial: warm paper, dark type, a blue accent |
| Harbor | Navy and brass with a serif heading |

Each has the same nine layouts, drawn for 16:9, 9:16, 1:1 and 4:5:

| Layout (`id`) | Kind | What it is |
|---|---|---|
| Speaker – Full (`speaker-full`) | camera | The picture fills the frame; a name plate widens with the name |
| Speaker – Framed (`speaker-framed`) | camera | The picture in a rounded window, over a caption |
| Speaker + Title (`speaker-title`) | camera | The picture on one side, a heading on the other |
| Screen + Speaker (`screen-speaker`) | split | A screen recording, shown whole, beside the speaker (stacked on tall frames) |
| Screen + Bubble (`screen-bubble`) | media | A screen recording fills the frame; the speaker rides in a round window |
| Title Card (`title`) | title | A heading and a line under it |
| Chapter Card (`chapter`) | chapter | A large chapter number with the chapter's title |
| Pull Quote (`quote`) | quote | A quotation, with the speaker in a round window beside their name |
| End Card (`outro`) | outro | A centred sign-off |

User packs live in `<data dir>/Layout Packs/`.

| Command | Menu | Params |
|---|---|---|
| `layout.pack.list` | (query) | `query`, `kind`, `source` (builtin / user): packs with their theme, smart transition and layouts (windows, properties, frame shapes) |
| `layout.pack.install` | Graphics and Titles ▸ Layouts ▸ Install Layout Pack… | `path` (a `.fcpack`) |
| `layout.pack.export` | Layouts ▸ Export Layout Pack… | `pack`, `path`: writes any pack, built-in ones too, as a file to read, edit and share |
| `layout.pack.remix` | Layouts ▸ Remix Layout Pack… | `pack`, `name`, `colors` `{theme colour: "#rrggbb"}`, `fonts` `{theme font: {family, style}}`, `transition`: saves the pack's layouts under a new look as a user pack |
| `layout.pack.remove` | | `pack` (a user pack) |
| `layout.save` | Layouts ▸ Save As Layout… | `clip`, `pack` (a user pack; made if missing), `name`, `kind`: adds a graphic clip to a pack as a layout. Saving a layout again from a sequence of another shape adds that shape's design to it |
| `layout.thumbnail` | (query) | `pack`, `layout`, `width`, `shape` `[w, h]`, `path` (PNG) |

## Scenes and applying a layout

A layout is applied to a **scene**: a stretch of the sequence. `layout.scenes` lists them: the
stretches between the sequence's chapter markers, or, when it has none, the clips of the lowest
video track that holds footage, one scene each.

`layout.apply` places a layout clip over a scene:

- **Where.** `scene` (a number from `layout.scenes`), `startSeconds` / `endSeconds` (or
  `…Frame`, `…Time`), or `clip` (a graphic clip to replace). Without any of them: the In and Out
  points, else the scene under the playhead.
- **Track.** Layout clips share one track, so that they can sit end to end. The first layout
  adds a track named *Layouts* above the sequence's tracks; later ones use the track that holds
  layouts. `track` chooses another. What that track had in the range is replaced: graphic clips
  inside it go, those reaching into it are trimmed, one spanning it is cut in two.
- **Windows.** Each media window is pointed at a track under the layout that holds footage in
  the scene, each at a different one. `map` decides first (`{"screen": "V2", "camera": 1}`, by
  window role or layer name); then tracks whose name, or whose clip's name, says what they hold
  ("Screen capture", "Cam A", "Interview"…); then order: camera windows take the lowest free
  track, the others the highest. A window that gets no footage shows its fill, and the result
  lists it under `unfilled`.
- **Text.** `values` sets the layout's properties (`{"heading": "Why it matters"}`). When the
  layout replaces another, text the old one showed is kept for the properties both have
  (`keepText: false` starts from the layout's own words).
- **Smart transition.** The clip takes the pack's; `transition` overrides it (`false`, or
  `{seconds, easing, fades}`).

It is one undo step. The placed clip is an ordinary graphic clip: its properties are edited in
Essential Graphics ▸ Edit (`graphics.template.set`), and every layer can be changed by hand.

| Command | Menu | Params |
|---|---|---|
| `layout.scenes` | (query) | scenes with their times and the layout on each |
| `layout.apply` | | `pack`, `layout`, `scene` \| `start…`, `end…` \| `clip`, `values`, `map`, `track`, `transition`, `keepText` |
| `layout.retheme` | Essential Graphics ▸ Layouts ▸ Restyle Sequence | `to` (take this pack's theme), `colors`, `fonts`, `pack` (only clips placed from this pack): restyles the layout clips already in the sequence, in one undo step |

## Smart transitions

When a layout clip starts exactly where another graphic clip ends on the same track, it can open
with a smart transition. For its duration, the picture is drawn from layers interpolated between
the outgoing layout's last frame and the incoming one:

- Layers that are **the same thing** in both move, resize and recolour into place: layers with
  the same name and kind, media windows with the same role, text with the same words. Position,
  size, corner radius, scale, rotation, opacity, fill, strokes, background and shadow are
  interpolated; a window keeps showing its live footage throughout.
- The **others** fade: out for the outgoing layout's, in for the incoming one's. With `fades`
  off they cut.

It is computed when each frame is drawn, from the two clips as they are: it writes no keyframes
and never goes stale when either layout is edited, retimed or swapped. Give layers the same name
in two layouts to make them travel (the built-in packs name theirs `Background`, `Speaker`,
`Screen`, `Accent`, `Heading`, `Subheading`).

| Setting | Meaning |
|---|---|
| `enabled` | Off: the layout cuts in |
| `seconds` (`frames`, `duration`) | How long the incoming clip takes to settle, from its first frame (default 0.5 s) |
| `easing` | `linear`, `easeIn`, `easeOut`, `easeInOut` (default), `overshoot` (runs slightly past and settles back) |
| `fades` | Fade the layers only one layout has (default on) |

| Command | Menu | Params |
|---|---|---|
| `layout.transition` | Essential Graphics ▸ Layouts (with a layout clip selected) | `clip` \| `all` (every layout clip of the sequence), `enabled`, `seconds` \| `frames`, `easing`, `fades` |

## The `.fcpack` format

A pack is one UTF-8 JSON file (`filmcraft_project::layoutpack::LayoutPack`):

```json
{
  "format": "filmcraft.layoutPack",
  "version": 1,
  "id": "user:my-pack",
  "name": "My Pack",
  "description": "", "author": "", "license": "CC0-1.0", "tags": [],
  "theme": {
    "colors": { "background": "#0f1115", "surface": "#1b1f27", "text": "#f5f6f8", "accent": "#ff5a36" },
    "fonts": { "heading": { "family": "Inter", "style": "Bold" } }
  },
  "transition": { "enabled": true, "duration": 127008000000, "easing": "easeInOut", "fades": true },
  "layouts": [
    {
      "id": "speaker-framed",
      "name": "Speaker – Framed",
      "kind": "camera",
      "description": "",
      "duration": 0,
      "controls": [ { "id": "heading", "name": "Heading", "kind": "text", "layer": 5, "param": "text" } ],
      "bindings": [ { "layer": 1, "param": "fill_color", "token": "color.background" },
                    { "layer": 5, "param": "font", "token": "font.heading" } ],
      "designs": [ { "canvas": [1920, 1080], "layers": [ /* graphic layers, as in a project file */ ] } ]
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `format`, `version` | `"filmcraft.layoutPack"`, `1` (newer versions are refused) |
| `id`, `name`, `description`, `author`, `license`, `tags` | library information. State the licence you share the pack under |
| `theme.colors` | named colours, `#rrggbb` or `#rrggbbaa` (sRGB) |
| `theme.fonts` | named fonts: `{family, style}` |
| `transition` | the smart transition given to the clips the pack places; `duration` in ticks (254 016 000 000 per second) |
| `layouts[].id` | unique in the pack: lower-case letters, digits, `-`, `_` |
| `layouts[].kind` | free text; the built-in packs use `camera`, `media`, `split`, `title`, `chapter`, `quote`, `outro`, `overlay` |
| `layouts[].duration` | clip length when placed without a range (ticks; 0 = five seconds). A layout applied to a scene takes the scene's length |
| `layouts[].graphic` | roll / crawl and responsive time of the placed clip, as for a graphics template |
| `layouts[].controls` | the properties a user of the layout edits, as in a graphics template: `{id, name, kind, layer, param, min, max}` |
| `layouts[].bindings` | `{layer, param, token}`: `token` is `color.<name>` for a colour parameter, or `font.<name>` with `param: "font"` (sets family and style). A themed colour keeps the alpha the layout gave the parameter |
| `layouts[].designs` | one or more drawings: `{canvas: [w, h], layers}`. `layers` are `graphic_text`, `graphic_shape` and `graphic_media` effect instances exactly as in a project file, each with a unique `layer.uid`; the same layer has the same uid in every design |
| `resources.fonts` | optional embedded font files (base64) with their licence, registered on install. Only embed fonts whose licence allows redistribution |

A pack file is untrusted input: sizes, counts and durations are bounded, ZIP archives are refused
without being inspected, and a file that fails validation is an error message, never a crash.

The quickest way to start a pack of your own is to export a built-in one
(`layout.pack.export`) and edit it, or to remix one (`layout.pack.remix`) and then save your
own clips into it (`layout.save`).

## Driving it from an agent

Every step is a command, so a whole video can be laid out without touching the mouse:

```sh
filmcraft-cli --project talk.fcproj exec layout.scenes
filmcraft-cli --project talk.fcproj --save exec layout.apply '{"pack":"Studio","layout":"title","scene":0,"values":{"heading":"Memory care units"}}'
filmcraft-cli --project talk.fcproj --save exec layout.apply '{"pack":"Studio","layout":"speaker-framed","scene":1,"values":{"heading":"What to ask on a tour"}}'
filmcraft-cli --project talk.fcproj --save exec layout.retheme '{"colors":{"accent":"#c8102e"}}'
```

The same commands run in the open app over the control channel and MCP ([agents.md](agents.md)).

## Essential Graphics ▸ Layouts

The Essential Graphics panel has a **Layouts** tab between Browse and Edit. It shows one pack's
layouts as cards drawn by the engine in the sequence's frame shape (a window without footage
shows a soft ramp), with a pack chooser and a kind filter. Click selects; double-click or **Apply
to Scene** lays out the scene under the playhead. **Restyle Sequence** gives every layout clip of
the sequence the chosen pack's look. **Remix…** opens a dialog with the pack's colours and fonts
and saves the result as a pack of your own; **Install…** adds a `.fcpack` file. With a layout
clip selected, its **Smart Transition** (on, duration, easing, fade) is edited under the cards.
Graphics and Titles ▸ Layouts holds Install, Export and Remix Layout Pack and Save As Layout.

Automation ids: `essentialGraphics.tab.layouts`, `layouts.pack`, `layouts.kind`,
`layouts.item.<layout id>`, `layouts.apply`, `layouts.restyle`, `layouts.install`,
`layouts.remix`, `layouts.transition.<enabled|seconds|easing|fades>`; dialogs:
`remixPack.name|color.<name>|font.<name>|ok|cancel`, `saveLayout.pack|name|kind|ok|cancel`;
`ui.set {"layouts": {...}}` sets the tab's state and `{"gfxTemplates": {"tab": "layouts"}}` shows it.

## Not yet

GPU compositing of media windows, a properties section for media layers in Essential Graphics ▸
Edit (they are edited with `graphics.set`), text that is written from the transcript, and smart
transitions out of a clip that is not a graphic.
