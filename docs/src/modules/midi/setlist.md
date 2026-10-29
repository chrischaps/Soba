# Setlist

**Module ID** `output.setlist` · **Category** Output

The [Arranger](../utilities/arranger.md) knows when the verse ends. A Setlist passes that on to the room. At the chorus, the Digitakt moves to pattern A05, the FANTOM recalls Scene 12, the ZOIA loads patch 4 and the RC-505 recalls memory 7.

Each section of the song has up to eight **rows**. A row is one device's program, sent as Bank Select and Program Change. A row with a **lead** goes that long before the bar line, so a device that needs warning still lands on the downbeat.

The Setlist makes no sound. The hardware changes.

## Patching it

Patch three of the Arranger's outputs into the Setlist's inputs of the same names:

- **Section**: which section is playing.
- **Next**: where the song goes at the next bar line. The Arranger knows this from the downbeat of a section's last bar, when its [Next](../utilities/arranger.md#wandering-where-a-section-goes-next) dice are thrown, or as soon as a Jump is asked for.
- **Bar**: measures how long a bar is, so a row knows when its lead starts.

With all three patched, every row goes at its lead before the bar line. Without **Bar**, a row with a lead goes as soon as **Next** names its section, which can be up to a bar early. Without **Next**, every row goes on the downbeat its section starts on, lead or not.

## Inputs

| Port | Signal Type | Description |
|------|-------------|-------------|
| **Section** | Control (Orange) | The Arranger's **Section**. On the downbeat a section starts, any of its rows that haven't gone ahead go now |
| **Next** | Control (Orange) | The Arranger's **Next**. The rows of the section it names go their lead ahead of the bar line |
| **Bar** | Gate (Green) | The Arranger's **Bar**, which measures the bar |
| **Send** | Gate (Green) | Sends the playing section's rows again as it rises, whether the transport is playing or stopped |

The module has no outputs and no knobs: its rows are all on its face.

## Rows

Each section's rows are listed under its name, as its Arranger names it. Click **+** beside a section to add a row. The new row copies the row in the same place in the section before it, moved on one program. If you start with the verse's Digitakt on A01, the chorus gets the Digitakt on A02. **Click a row** to change it:

| Setting | What it is |
|---------|------------|
| **Device** | One of the [instrument profiles](../../hardware/instruments.md) with programs, or **Plain numbers**. Choosing a device sets the row's channel to the one the device takes programs on, and sets its port to the port whose device looks like it |
| **Port** | **A** to **H**: which device it goes to, as [MIDI Out's ports](../output/midi-out.md#ports-not-devices) do. Hover over a port to see its device |
| **Channel** | 1 – 16. For a device banked by channel (the SP-404MKII), this is the first bank's channel |
| **Bank** | The device's banks (the FANTOM's Scene A and Scene B), or for plain numbers, Bank Select MSB and LSB, each 0 – 127 or off |
| **Program** | Named the way the device names it: **Pattern A05**, **Scene 12**, **Memory 7**. You can type a name ("B16") or drag. A plain row shows the Program Change number as it's sent, 0 – 127 |
| **Lead** | 0 – 4000 ms: how long before the bar line it goes |

**Remove row** at the bottom takes the row away. Every change is one undo step.

### Leads

Most devices switch as soon as they hear a program change, so give them a lead of **0** and they change on the downbeat.

A Digitakt doesn't. It queues its next pattern and switches when the one playing ends. Give its row a lead inside the last bar, 500 ms or so: the pattern is queued during the last bar and starts on the downbeat. A row's lead shows in orange at its right.

## The display

- **Along the top**: a lamp that flashes purple as rows go, and the Arranger it follows. While a section's rows are on their way, the time to the bar line shows at the right: **bar in 0.4 s**.
- **Each section** has a tab beside its name. The tab is **green** when the room is playing that section, meaning all its rows have gone. It's **amber**, pulsing, while that section's rows are going ahead of its bar line. A section with no rows says **no change**: the room stays as it is.
- **Each row** shows a lamp that glows as the row goes, then its port, then the device and its program, then its lead. The port is lit the way its port is in [the Rig](../../getting-started/interface-overview.md#the-rig): green when sending, amber when the port has no device or it's unplugged, and red when the device won't open.

Hover over a row to see exactly what it sends: **Pattern A05: Program Change 4 on channel 1, port A, 500 ms before the bar line**.

## How it works

**Only while playing.** Rows go only while the [Clock](../modulation/clock.md) runs. When the Clock starts, the section that's coming first goes at once, before its downbeat. That's section 1 from the top, or wherever a Jump starts the song. Stopping and starting again sends the section again.

**A change of plan.** Say the chorus's rows have gone ahead, and then a Jump sends the song back to the verse before the bar line. The verse's rows go next, at their leads, so the room follows the song back.

**The profile is read, not copied.** As an [Instrument's](./instrument.md) knobs do, a row naming a device goes as its profile says: the channel a bank is on, and the bank's Bank Select. If a number is corrected in a profile, every patch that uses it gets the correction. The patch also writes each row down, so it still plays on a computer without the profile. There, a row sends its program as the Program Change, on its channel, after the bank it wrote down.

**On time.** Each program change is stamped with its sample and goes out as that sample is heard, the way [MIDI Out's](../output/midi-out.md#how-it-works) messages do. A busy port's budget never holds one back. A port no row uses is left alone, so the Rig doesn't show it as in use.

## Playing it

- **Three patterns on the bar.** An Arranger with Intro, Verse and Chorus, each with a row for the **Digitakt** on its own pattern with a 500 ms lead. The Digitakt moves through the three patterns, each exactly on its bar line.
- **A scene for the chorus.** A row for the **FANTOM-06** in the Chorus, Bank **Scene A**, Scene 12. The FANTOM changes scene on the chorus's downbeat.
- **Put the room back.** Patch a [Pad](./pad.md)'s gate into **Send**. After you've been turning things on the hardware, one hit brings every device back to the section playing.

## Related

- [Arranger](../utilities/arranger.md): the song the Setlist follows, and its **Next** output
- [Instrument Profiles](../../hardware/instruments.md): how each device's programs are named and numbered
- [MIDI Out](../output/midi-out.md): a single program and bank, sent on a trigger
