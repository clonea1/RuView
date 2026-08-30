#!/usr/bin/env python3
"""Generate a printable enclosure (STL) for an ESP32 sensing node.

The RF constraint drives the design
-----------------------------------
A PCB trace antenna is detuned by dielectric near it. Plastic pressed against
the antenna shifts its resonance and costs gain -- permanently, and identically
on every node, which is the worst kind of error because it looks like a
property of the room rather than of the case.

This fleet has already measured what obstruction costs: wire cages flanking one
node put ~13 dB of excess loss on its shortest link. A bad case is the same
class of problem, self-inflicted.

So the antenna is pushed OUT of the box rather than given room inside it. The
enclosure is otherwise a closed tray: the antenna end carries a full wall with
a horizontal slot at board height, and the last ANTENNA_CLEAR_MM of the board
slides through and sits in free air. Nothing dielectric ends up beside the
antenna, and the electronics are still fully housed -- better than truncating
the case, which left the board's end unsupported and open to dust.

Print notes
-----------
PLA or PETG. **No carbon-fill, no metal-fill, no conductive filament** -- carbon
is lossy at 2.4 GHz and would quietly attenuate every link.
No supports needed: the design is a tray with vertical walls.
0.2 mm layers, 3 perimeters, 20% infill is plenty.

    python make_case.py                     # defaults, writes node-case.stl
    python make_case.py --pcb-l 52 --pcb-w 25.5 --pcb-h 1.6
    python make_case.py --lid               # also write a lid

Measure three numbers off your board and pass them: length, width, and the
distance from the antenna end to where the antenna region ends. If unsure,
over-estimate the antenna clearance -- an oversized opening costs nothing, a
tight one costs dB on every link forever.
"""

import argparse
import struct

# ---- geometry helpers ----------------------------------------------------
# Everything is built from axis-aligned boxes and emitted as triangles, so no
# CSG library is needed and the output is always a closed, printable mesh.

def box(x0, y0, z0, x1, y1, z1):
    """Twelve triangles, outward normals, for an axis-aligned box."""
    p = [(x0,y0,z0),(x1,y0,z0),(x1,y1,z0),(x0,y1,z0),
         (x0,y0,z1),(x1,y0,z1),(x1,y1,z1),(x0,y1,z1)]
    q = [(0,3,2,1),(4,5,6,7),(0,1,5,4),(2,3,7,6),(1,2,6,5),(0,4,7,3)]
    tris = []
    for a,b,c,d in q:
        tris.append((p[a],p[b],p[c]))
        tris.append((p[a],p[c],p[d]))
    return tris


def normal(t):
    (ax,ay,az),(bx,by,bz),(cx,cy,cz) = t
    ux,uy,uz = bx-ax, by-ay, bz-az
    vx,vy,vz = cx-ax, cy-ay, cz-az
    nx,ny,nz = uy*vz-uz*vy, uz*vx-ux*vz, ux*vy-uy*vx
    m = (nx*nx+ny*ny+nz*nz) ** 0.5 or 1.0
    return (nx/m, ny/m, nz/m)


def write_stl(path, tris, name="node_case"):
    with open(path, "wb") as f:
        f.write(b"\0" * 80)
        f.write(struct.pack("<I", len(tris)))
        for t in tris:
            f.write(struct.pack("<3f", *normal(t)))
            for v in t:
                f.write(struct.pack("<3f", *v))
            f.write(struct.pack("<H", 0))
    return len(tris)


# ---- the case ------------------------------------------------------------

def build(a):
    W = a.wall
    ix = a.pcb_l + 2*a.fit
    iy = a.pcb_w + 2*a.fit
    iz = a.standoff + a.pcb_h + a.headroom

    # How much board is enclosed; the rest protrudes through the slot.
    enc = ix - a.antenna_clear
    outer_x = enc + 2*W
    outer_y = iy + 2*W

    # Slot geometry: the board passes through at its seated height, with a
    # little clearance so it is not a press fit through a printed edge.
    g = a.slot_gap
    sz0 = W + a.standoff - g
    sz1 = W + a.standoff + a.pcb_h + g
    sy0 = W - g
    sy1 = W + iy + g

    t = []
    t += box(0, 0, 0, outer_x, outer_y, W)                       # floor

    # Long walls, segmented for ventilation. The radio runs continuously.
    seg = outer_x / (2*a.vents + 1)
    for i in range(a.vents + 1):
        x0, x1 = i*2*seg, min(i*2*seg + seg, outer_x)
        if x1 <= x0:
            continue
        t += box(x0, 0, 0, x1, W, W + iz)
        t += box(x0, outer_y - W, 0, x1, outer_y, W + iz)

    # Closed end, with the USB-C opening left as a gap in the wall.
    u0 = (outer_y - a.usb_w) / 2
    t += box(0, 0, 0, W, u0, W + iz)
    t += box(0, u0 + a.usb_w, 0, W, u0 + a.usb_w + (outer_y - u0 - a.usb_w), W + iz)
    t += box(0, u0, W + a.usb_h, W, u0 + a.usb_w, W + iz)

    # Antenna end: a full wall, slotted so the board passes through.
    ax0, ax1 = outer_x - W, outer_x
    t += box(ax0, 0, 0, ax1, outer_y, sz0)                       # under the slot
    t += box(ax0, 0, sz1, ax1, outer_y, W + iz)                  # over the slot
    t += box(ax0, 0, sz0, ax1, sy0, sz1)                         # beside, near
    t += box(ax0, sy1, sz0, ax1, outer_y, sz1)                   # beside, far

    # Standoffs.
    for sx in (W + 3, outer_x - W - 6):
        for sy in (W + 3, outer_y - W - 3 - a.pad):
            t += box(sx, sy, W, sx + a.pad, sy + a.pad, W + a.standoff)

    # Retaining lip at the closed end so the board cannot back out of the slot.
    t += box(W, W, W + a.standoff + a.pcb_h,
             W + 2, outer_y - W, W + a.standoff + a.pcb_h + 1.2)

    # Mounting tabs on the long sides, clear of the antenna.
    for sy, sy2 in ((0 - a.tab_l, 0), (outer_y, outer_y + a.tab_l)):
        t += box(outer_x*0.25, sy, 0, outer_x*0.25 + a.tab_w, sy2, W)

    return t, (outer_x, outer_y + 2*a.tab_l, W + iz)


def build_lid(a):
    W = a.wall
    ix = a.pcb_l + 2*a.fit
    iy = a.pcb_w + 2*a.fit
    enc = ix - a.antenna_clear
    outer_x = enc + 2*W
    outer_y = iy + 2*W
    t = []
    t += box(0, 0, 0, outer_x, outer_y, W)
    # Inner lip so it locates without fasteners.
    t += box(W + 0.3, W + 0.3, W, outer_x - W - 0.3, outer_y - W - 0.3, W + 2)
    return t, (outer_x, outer_y, W + 2)


def main():
    p = argparse.ArgumentParser(description=__doc__,
            formatter_class=argparse.RawDescriptionHelpFormatter)
    # Defaults are ESP32-C6-DevKitC-1. MEASURE YOURS.
    p.add_argument("--pcb-l", type=float, default=52.0, help="board length mm")
    p.add_argument("--pcb-w", type=float, default=25.5, help="board width mm")
    p.add_argument("--pcb-h", type=float, default=1.6,  help="board thickness mm")
    p.add_argument("--antenna-clear", type=float, default=18.0,
                   help="mm of board protruding through the slot, antenna end")
    p.add_argument("--slot-gap", type=float, default=0.6,
                   help="clearance around the board in the antenna slot")
    p.add_argument("--wall", type=float, default=2.0)
    p.add_argument("--fit", type=float, default=0.4, help="clearance per side")
    p.add_argument("--standoff", type=float, default=2.5)
    p.add_argument("--headroom", type=float, default=8.0)
    p.add_argument("--usb-w", type=float, default=10.0)
    p.add_argument("--usb-h", type=float, default=4.0)
    p.add_argument("--pad", type=float, default=3.0)
    p.add_argument("--vents", type=int, default=2)
    p.add_argument("--tab-l", type=float, default=8.0)
    p.add_argument("--tab-w", type=float, default=8.0)
    p.add_argument("--out", default="node-case.stl")
    p.add_argument("--lid", action="store_true", help="also write node-lid.stl")
    a = p.parse_args()

    tris, dims = build(a)
    n = write_stl(a.out, tris)
    print("%s  %d triangles  %.1f x %.1f x %.1f mm" % (a.out, n, *dims))
    if a.lid:
        lt, ld = build_lid(a)
        ln = write_stl("node-lid.stl", lt)
        print("node-lid.stl  %d triangles  %.1f x %.1f x %.1f mm" % (ln, *ld))
    print("\nantenna slot: %.1f mm of board protrudes into free air" % a.antenna_clear)
    print("print in PLA or PETG. NOT carbon-fill or any conductive filament:")
    print("carbon is lossy at 2.4 GHz and would attenuate every link.")


if __name__ == "__main__":
    main()
