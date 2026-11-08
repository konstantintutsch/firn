"""Routes the made-up activities follow, snapped to real streets and paths.

Run once with `python3 dev/demo/build_routes.py` to write routes.json, which
fake_garmin.py reads. Only waypoints are listed here. A public routing service
(OpenStreetMap data) finds the way between them, so the tracks follow roads and
paths like recorded ones do. The result is committed, so the demo needs no
network to start.
"""

import json
import math
import random
import sys
import time
import urllib.request
from pathlib import Path

# Foot routing finds park paths and riverside trails, bike routing finds roads.
SERVICES = {
    "foot": "https://routing.openstreetmap.de/routed-foot/route/v1/foot",
    "bike": "https://routing.openstreetmap.de/routed-bike/route/v1/driving",
}


def loop(*points):
    """Through the waypoints and back to the first."""
    return ("loop", [*points, points[0]])


def out_and_back(*points):
    """Through the waypoints, then back the same way."""
    return ("out_and_back", list(points))


# Waypoints as (latitude, longitude).
PLACES = {
    "Berlin": {
        "routes": {
            "Tiergarten": loop(
                (52.5145, 13.3501), (52.5163, 13.3640), (52.5162, 13.3770), (52.5122, 13.3770),
                (52.5095, 13.3650), (52.5098, 13.3480), (52.5120, 13.3390),
            ),
            "Tempelhofer Feld": loop(
                (52.4805, 13.3905), (52.4810, 13.4040), (52.4770, 13.4170), (52.4690, 13.4150),
                (52.4675, 13.4010), (52.4720, 13.3920),
            ),
            "Spree": loop(
                (52.5190, 13.3700), (52.5205, 13.3900), (52.5170, 13.4080), (52.5110, 13.4200),
                (52.5030, 13.4380), (52.5015, 13.4455), (52.5040, 13.4400), (52.5120, 13.4230),
                (52.5180, 13.4090), (52.5212, 13.3880),
            ),
            "Grunewald": loop(
                (52.4890, 13.2610), (52.4800, 13.2480), (52.4660, 13.2420), (52.4560, 13.2310),
                (52.4480, 13.2450), (52.4600, 13.2620), (52.4760, 13.2700),
            ),
            "Landwehrkanal": out_and_back(
                (52.5045, 13.3650), (52.5010, 13.3800), (52.4985, 13.3950), (52.4960, 13.4100),
                (52.4930, 13.4250), (52.4950, 13.4350),
            ),
            "Volkspark Friedrichshain": loop(
                (52.5280, 13.4280), (52.5300, 13.4370), (52.5260, 13.4450), (52.5220, 13.4380),
                (52.5235, 13.4300),
            ),
            "Treptower Park": loop(
                (52.4930, 13.4600), (52.4900, 13.4720), (52.4840, 13.4800), (52.4820, 13.4700),
                (52.4870, 13.4610),
            ),
            "Mauerweg": out_and_back(
                (52.5440, 13.4020), (52.5400, 13.3960), (52.5350, 13.3890), (52.5300, 13.3820),
                (52.5250, 13.3760),
            ),
            "Hasenheide": loop(
                (52.4880, 13.4010), (52.4840, 13.4140), (52.4800, 13.4200), (52.4790, 13.4080),
                (52.4830, 13.3990),
            ),
            "Gleisdreieck": loop(
                (52.4990, 13.3740), (52.4930, 13.3730), (52.4880, 13.3680), (52.4950, 13.3600),
                (52.5010, 13.3640),
            ),
            "Prenzlauer Berg": loop(
                (52.5440, 13.4050), (52.5480, 13.4180), (52.5400, 13.4260), (52.5350, 13.4160),
                (52.5370, 13.4060),
            ),
            "Schlachtensee": loop(
                (52.4400, 13.2150), (52.4350, 13.2240), (52.4300, 13.2300), (52.4340, 13.2180),
            ),
            "Humboldthain": loop(
                (52.5470, 13.3830), (52.5460, 13.3920), (52.5410, 13.3900), (52.5400, 13.3800),
            ),
        },
        # Long rides out of town and back. Runs never take them.
        "rides": {
            "Potsdam": out_and_back(
                (52.5145, 13.3501), (52.4880, 13.2700), (52.4500, 13.2000), (52.4200, 13.1300),
                (52.4000, 13.0600), (52.3950, 13.0450),
            ),
            "Müggelsee": out_and_back(
                (52.4900, 13.4700), (52.4600, 13.5500), (52.4450, 13.6300), (52.4350, 13.6600),
            ),
        },
    },
    "Hamburg": {
        "routes": {
            "Alster": loop(
                (53.5560, 10.0010), (53.5620, 9.9970), (53.5700, 9.9990), (53.5770, 10.0040),
                (53.5760, 10.0130), (53.5680, 10.0150), (53.5600, 10.0100),
            ),
            "Stadtpark": loop(
                (53.5960, 10.0190), (53.5990, 10.0300), (53.5960, 10.0420), (53.5900, 10.0360),
                (53.5910, 10.0230),
            ),
        },
    },
    "Munich": {
        "routes": {
            "Englischer Garten": loop(
                (48.1450, 11.5880), (48.1530, 11.5930), (48.1620, 11.5960), (48.1720, 11.6020),
                (48.1680, 11.6090), (48.1570, 11.6020), (48.1480, 11.5960),
            ),
            "Isar": loop(
                (48.1300, 11.5780), (48.1220, 11.5700), (48.1100, 11.5600), (48.0980, 11.5480),
                (48.1060, 11.5560), (48.1180, 11.5680),
            ),
        },
    },
    "Mallorca": {
        "rides": {
            "Tramuntana": out_and_back(
                (39.5696, 2.6502), (39.6290, 2.6830), (39.7107, 2.6224), (39.7487, 2.6467),
            ),
        },
    },
}


# Places to start from, as (name, latitude, longitude). Loops of random shape
# around them fill the street grid, so that the heatmap has a network to show.
HUBS = [
    ("Mitte", 52.5200, 13.4050), ("Kreuzberg", 52.4990, 13.4030),
    ("Prenzlauer Berg", 52.5380, 13.4140), ("Friedrichshain", 52.5130, 13.4540),
    ("Charlottenburg", 52.5160, 13.3040), ("Neukölln", 52.4810, 13.4380),
    ("Moabit", 52.5300, 13.3430), ("Schöneberg", 52.4850, 13.3550),
]
LOOPS_PER_HUB = 6


def random_loop(hub, rng):
    """Three or four waypoints in a ring around the hub, in order around it."""
    _, lat, lon = hub
    radius = rng.uniform(1200, 2800)
    count = rng.choice([3, 4])
    start = rng.uniform(0, math.tau)
    points = []
    for index in range(count):
        angle = start + math.tau * (index + rng.uniform(-0.2, 0.2)) / count
        distance = radius * rng.uniform(0.7, 1.2)
        points.append((
            round(lat + distance * math.sin(angle) / 111_320, 5),
            round(lon + distance * math.cos(angle) / (111_320 * math.cos(math.radians(lat))), 5),
        ))
    return loop(*points)


def meters(a, b):
    dy = (b[0] - a[0]) * 111_320
    dx = (b[1] - a[1]) * 111_320 * math.cos(math.radians(a[0]))
    return math.hypot(dx, dy)


def without_spurs(path):
    """The path without detours into a dead end and back out beside the way in."""
    kept = []
    for point in path:
        if len(kept) >= 2 and kept[-2] == point:
            kept.pop()
        else:
            kept.append(point)
    index = 0
    while index < len(kept):
        for end in range(min(len(kept) - 1, index + 60), index + 2, -1):
            if meters(kept[index], kept[end]) > 15:
                continue
            excursion = kept[index:end + 1]
            far = max(range(len(excursion)), key=lambda i: meters(excursion[0], excursion[i]))
            outward, back = excursion[:far + 1], excursion[far:]
            # A spur comes back along the way it went. A block around a corner does not.
            if 0 < far < len(excursion) - 1 and all(
                min(meters(point, other) for other in outward) < 15 for point in back
            ):
                kept = kept[:index + 1] + kept[end + 1:]
                break
        index += 1
    return kept


def route(profile, waypoints):
    coordinates = ";".join(f"{lon},{lat}" for lat, lon in waypoints)
    url = f"{SERVICES[profile]}/{coordinates}?overview=full&geometries=geojson"
    with urllib.request.urlopen(url, timeout=60) as response:
        body = json.load(response)
    if body["code"] != "Ok":
        raise RuntimeError(f"{body['code']} for {waypoints}")
    path = []
    for lon, lat in body["routes"][0]["geometry"]["coordinates"]:
        point = [round(lat, 6), round(lon, 6)]
        if not path or path[-1] != point:
            path.append(point)
    return without_spurs(path), body["routes"][0]["distance"]


def main():
    rng = random.Random(3)
    generated = set()
    for hub in HUBS:
        for number in range(1, LOOPS_PER_HUB + 1):
            name = f"{hub[0]} {number}"
            PLACES["Berlin"]["routes"][name] = random_loop(hub, rng)
            generated.add(name)
    result = {}
    for city, place in PLACES.items():
        result[city] = {"routes": {}, "rides": {}}
        for group, profile in (("routes", "foot"), ("rides", "bike")):
            for name, (kind, waypoints) in place.get(group, {}).items():
                path, distance = route(profile, waypoints)
                if name in generated and not 4000 <= distance <= 20000:
                    print(f"{city} / {name}: skipped, {distance / 1000:.1f} km", file=sys.stderr)
                    continue
                if kind == "out_and_back":
                    path = path + path[-2::-1]
                    distance *= 2
                result[city][group][name] = path
                print(f"{city} / {name}: {distance / 1000:.1f} km, {len(path)} points", file=sys.stderr)
                time.sleep(1)
    Path(__file__).with_name("routes.json").write_text(json.dumps(result, separators=(",", ":")) + "\n")


main()
