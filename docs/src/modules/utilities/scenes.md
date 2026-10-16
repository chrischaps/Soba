# Scenes

**Module ID** `util.scenes` · **Category** Utility

Scenes keeps the knobs of a patch as **scenes**, A to H, and glides between two of them with one knob. A verse and its breakdown, a dry sound and a drenched one, the calm before a drop: store each as a scene, and **Morph** carries every knob from one to the other at once. A change that would take eight hands becomes one gesture, and the space halfway between two scenes, where nobody set the knobs on purpose, is often where the good sounds are.

The scenes live on the module, so you can see them in the patch, and they're saved with it.

## What a scene holds

A Scenes module at the top of the patch holds every knob and switch in the patch. Put one **inside a group**, and it holds only that group's knobs (and those of the groups inside it), so a voice can have scenes of its own without touching the drums.

A scene leaves out the Audio Output, other Scenes modules, the keys a Keyboard or MIDI Note plays, and switches that start, stop or record rather than shape the sound (a Clock's **Run**, a Sampler's **Rec**, an Audio Input's **Channel**).

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Morph** | Control (Orange) | Replaces the Morph knob: 0 is the From scene, 1 the To scene. Patch an LFO (with **Bipolar** off), an envelope or an [Arranger](./arranger.md) lane |

## Parameters

| Control | Range | Default | Description |
|---------|-------|---------|-------------|
| **Morph** | 0 to 1 | 0 | How far from the From scene to the To scene the knobs sit |
| **From** | A to H | A | The scene at Morph 0 |
| **To** | A to H | B | The scene at Morph 1 |

## The pads

Each pad is a scene. A pad with a scene in it draws the scene's knobs as a row of ticks, each as high as the knob is turned. The pad the knobs stand at right now is ringed in blue.

- **Click an empty pad** to keep the knobs as they stand.
- **Click a full pad** to recall it: every knob it holds jumps there.
- **Shift+click a full pad** to keep the knobs there instead.
- **Alt+click a full pad** to clear it.

The From and To pads glow, each in proportion to how near Morph has carried the knobs to it, so the light crossfades from one to the other as Morph turns. Along the bottom, a dot rides a line from From to To. The header says what the module reaches (**PATCH**, or its group's name) and where Morph is.

Keeping, recalling and clearing a scene are each one undo step.

## Morphing

As Morph moves, each knob the two scenes share moves from its place in one to its place in the other:

- **Knobs glide round their own travel**, as a hand would turn them. A frequency knob moves by octaves, so halfway between 200 Hz and 3200 Hz is 800 Hz, not 1700.
- **Whole-number knobs, switches and dropdowns** hold the From scene's setting until Morph passes halfway, then flip to the To scene's.
- **A knob a cable drives** is left to the cable.
- **A knob only one scene holds** (on a module added after the other was kept) stays where it is.

The knobs turn on screen as Morph moves, and reach the sound through the same smoothing as a hand on the knob, so a glide has no zipper noise, even when an LFO drives it.

Morph only moves the knobs when it moves. Turn a knob by hand after a glide and it stays where you put it until Morph moves again. Undo works the same way: undoing a turn of Morph (or a change of From or To) puts the knobs back with it.

### Driving Morph

- **By hand:** turn Morph, or map it to a MIDI controller (right-click it, **MIDI Learn**) and ride it.
- **An LFO:** a very slow sine with **Bipolar** off breathes between two scenes. At 0.03 Hz it takes 32 seconds to go there and back.
- **The Arranger:** patch a lane into Morph, and give each section a level. **Jump** switches scenes on the downbeat; **Ramp** glides over the section, for a build.
- **An envelope:** a long ADSR swells into a scene and back as a key is held.

## Keeping a mutation

[Mutate](../../getting-started/interface-overview.md#mutate) wanders the knobs at random. When it lands somewhere you like, press `Ctrl + Shift + M` to keep the knobs in the next free scene of the Scenes module that reaches the selection (the one in its group, else the patch's). Then set From to the scene you started at and To to the new one, and Morph finds everything between them.

## Patches

The [Breakdown](../../recipes/breakdown.md) recipe glides an acid line between a verse and a breakdown.

## Tips

- **Keep a home scene.** Store A before you start exploring, so you can always click your way back.
- **Morph between near neighbours.** Two scenes far apart sweep everything at once; two close ones make a subtle, living change.
- **Several Scenes modules** can share a patch: one inside each group, each with its own Morph, and one at the top for the whole song.
- **In the render tool,** the knobs sit where they were saved: the glide is the editor's, and an offline render doesn't move Morph's knobs.
