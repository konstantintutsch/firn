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

# Where most runs start, as (latitude, longitude).
HOME = (52.5010, 13.4180)


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
                HOME, (52.4880, 13.2700), (52.4500, 13.2000), (52.4200, 13.1300),
                (52.4000, 13.0600), (52.3950, 13.0450),
            ),
            "Müggelsee": out_and_back(
                HOME, (52.4600, 13.5500), (52.4450, 13.6300), (52.4350, 13.6600),
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


# Most runs start at the front door, like they do for most people. Loops of
# random shape that all begin and end there make the heat fall off with the
# distance from home, and the street grid around it show.
HOME_LOOPS = 12


def home_loop(rng):
    """Home, then two or three waypoints around it, and back. Mostly short ones."""
    radius = 700 + 3300 * rng.random() ** 2
    count = rng.choice([2, 3])
    start = rng.uniform(0, math.tau)
    points = [HOME]
    for index in range(count):
        angle = start + math.tau * (index + 0.5 + rng.uniform(-0.2, 0.2)) / (count + 1)
        distance = radius * rng.uniform(0.7, 1.2)
        points.append((
            round(HOME[0] + distance * math.sin(angle) / 111_320, 5),
            round(HOME[1] + distance * math.cos(angle) / (111_320 * math.cos(math.radians(HOME[0]))), 5),
        ))
    return loop(*points)


# Where easy runs go, and what the long runs and speed work do with it. The
# main route is run almost every time. Speed work runs only the first part of it
# and back, which fake_garmin.py cuts from the main route. The long runs share
# its first waypoints, so they match it for a while, and then branch off
# further out. The long run east out and back is cut at different distances.
MAIN_OUT = [(52.4960, 13.4470), (52.4890, 13.4720)]
HOME_ROUTES = {
    "Home Main": loop(HOME, *MAIN_OUT),
    "Home Long East": out_and_back(
        HOME, *MAIN_OUT, (52.4570, 13.5190), (52.4480, 13.5800), (52.4350, 13.6400),
    ),
    "Home Long 1": loop(
        HOME, *MAIN_OUT, (52.4650, 13.4980), (52.4700, 13.4500), (52.4830, 13.4300),
    ),
    "Home Long 2": loop(
        HOME, *MAIN_OUT, (52.4700, 13.5200), (52.4480, 13.5750), (52.4400, 13.5450),
        (52.4560, 13.4950), (52.4760, 13.4450),
    ),
    "Home Long 3": loop(
        HOME, (52.4985, 13.3950), (52.5030, 13.3700), (52.5145, 13.3501), (52.5200, 13.3900),
        (52.5120, 13.4220),
    ),
}

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
    PLACES["Berlin"]["routes"].update(HOME_ROUTES)
    for number in range(1, HOME_LOOPS + 1):
        name = f"Home {number}"
        PLACES["Berlin"]["routes"][name] = home_loop(rng)
        generated.add(name)
    # Names given on the command line are the only ones asked for again.
    only = set(sys.argv[1:])
    target = Path(__file__).with_name("routes.json")
    old = json.loads(target.read_text()) if only and target.exists() else {}
    result = {}
    for city, place in PLACES.items():
        result[city] = {"routes": {}, "rides": {}}
        for group, profile in (("routes", "foot"), ("rides", "bike")):
            for name, (kind, waypoints) in place.get(group, {}).items():
                if only and name not in only:
                    if name in old.get(city, {}).get(group, {}):
                        result[city][group][name] = old[city][group][name]
                    continue
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
    target.write_text(json.dumps(result, separators=(",", ":")) + "\n")


main()
