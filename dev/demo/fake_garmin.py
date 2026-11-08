"""A stand-in for the Garmin Personal API that serves made-up data.

Runmap syncs from it like from Garmin, so the demo shows every page without
anyone's real activities. Everything comes from a fixed seed, so the data is
the same on every start, apart from moving along with today's date.

Only the Python standard library, so that it runs in a plain python image.
"""

import bisect
import io
import json
import math
import random
import struct
import zipfile
import zlib
from datetime import date, datetime, time, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

SEED = 7
DAYS = 730
PORT = 8000
# The address the backend reaches this server at, for the photo links.
PUBLIC_BASE = "http://fake-garmin:8000"

# Routes follow real streets and paths, see build_routes.py. Each is a list of
# (latitude, longitude) points and ends where it starts.
ROUTES = json.loads((Path(__file__).with_name("routes.json")).read_text())

PLACES = {
    "Berlin": {
        "timezone": 1,
        "elevation": 40,
        # How often each of the other routes is taken. The home routes get 3.
        "weights": {"Tiergarten": 3, "Spree": 2, "Landwehrkanal": 2},
    },
    "Hamburg": {"timezone": 1, "elevation": 25},
    "Munich": {"timezone": 1, "elevation": 520},
    "Mallorca": {"timezone": 1, "elevation": 120},
}
# The routes of the training runs from home, see build_routes.py. Easy runs take the
# main route, speed work its first part and back, and long runs go on from there.
HOME_MAIN = "Home Main"
HOME_LONG = ["Home Long East", "Home Long 1", "Home Long 2", "Home Long 3"]
HOME_SPECIAL = {HOME_MAIN, *HOME_LONG}
INTERVALS = [
    "Intervals: 6 x 800 m.",
    "Intervals: 5 x 1 km.",
    "Speed work: 10 x 400 m.",
    "Tempo: 3 x 2 km.",
]
for _city, _place in PLACES.items():
    _place["routes"] = ROUTES[_city]["routes"]
    _place["rides"] = ROUTES[_city]["rides"]

# Activity types with GPS: (type, share, speed in m/s, distance range in km).
GPS_TYPES = [
    ("running", 0.55, 3.0, (5, 16)),
    ("trail_running", 0.05, 2.5, (8, 20)),
    ("cycling", 0.22, 7.2, (25, 90)),
    ("walking", 0.08, 1.4, (3, 8)),
    ("hiking", 0.05, 1.2, (8, 18)),
]
# Activity types without GPS: (type, share, minutes).
INDOOR_TYPES = [
    ("strength_training", 0.6, (30, 60)),
    ("indoor_cycling", 0.25, (40, 75)),
    ("treadmill_running", 0.15, (25, 50)),
]

DESCRIPTIONS = [
    "Easy pace, felt good.",
    "Windy along the water.",
    "Intervals: 6 x 800 m.",
    "Long and slow.",
    "With friends.",
    "Legs were heavy today.",
]
FIT_EPOCH = datetime(1989, 12, 31, tzinfo=timezone.utc)


# --- Geometry -----------------------------------------------------------------


def meters(a, b):
    lat = math.radians((a[0] + b[0]) / 2)
    dy = (b[0] - a[0]) * 111_320
    dx = (b[1] - a[1]) * 111_320 * math.cos(lat)
    return math.hypot(dx, dy)


def walk(loop, distance, rng, from_start=False, forward=False):
    """Positions every meter along `loop`, round and round, for `distance` meters.

    Runs from home and rides begin at the first point, other runs anywhere on the loop.
    A race goes `forward` from the first point.
    The positions are slightly off the street, like a watch records them: a
    constant offset per run plus a slow wobble.
    """
    offset = (rng.gauss(0, 1.0), rng.gauss(0, 1.0))
    wobble = [0.0, 0.0]
    # From home, mostly the same way round.
    reverse = rng.random() < (0.2 if from_start else 0.5) and not forward
    loop = list(reversed(loop)) if reverse else loop
    start = 0 if from_start or forward else rng.randrange(len(loop))
    loop = loop[start:] + loop[:start]
    position = 0.0
    index = 0
    current = loop[0]
    while position < distance:
        target = loop[(index + 1) % len(loop)]
        step = meters(current, target)
        if step < 1e-6:
            index += 1
            continue
        # North and east in meters, correlated from one meter to the next.
        wobble = [0.9 * value + rng.gauss(0, 0.55) for value in wobble]
        north = offset[0] + wobble[0]
        east = offset[1] + wobble[1]
        yield (current[0] + north / 111_320,
               current[1] + east / (111_320 * math.cos(math.radians(current[0])))), position
        fraction = min(1.0, 1.0 / step)
        current = (current[0] + (target[0] - current[0]) * fraction,
                   current[1] + (target[1] - current[1]) * fraction)
        position += min(1.0, step)
        if fraction >= 1.0:
            index += 1


# --- Encoding -----------------------------------------------------------------


def fit_crc(data, crc=0):
    table = [0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401,
             0xA001, 0x6C00, 0x7800, 0xB401, 0x5000, 0x9C01, 0x8801, 0x4400]
    for byte in data:
        tmp = table[crc & 0xF]
        crc = (crc >> 4) & 0x0FFF
        crc = crc ^ tmp ^ table[byte & 0xF]
        tmp = table[crc & 0xF]
        crc = (crc >> 4) & 0x0FFF
        crc = crc ^ tmp ^ table[(byte >> 4) & 0xF]
    return crc


def fit_file(samples, vo2_max=None):
    """A FIT file with one record message per sample, and the VO2 max if given."""
    # Field number, size, base type.
    fields = [
        (253, 4, 0x86),  # timestamp
        (0, 4, 0x85),  # position_lat
        (1, 4, 0x85),  # position_long
        (2, 2, 0x84),  # altitude: (m + 500) * 5
        (3, 1, 0x02),  # heart_rate
        (4, 1, 0x02),  # cadence
        (5, 4, 0x86),  # distance: cm
        (6, 2, 0x84),  # speed: mm/s
    ]
    data = bytearray([0x40, 0, 0])
    data += struct.pack("<H", 20)
    data.append(len(fields))
    for number, size, base in fields:
        data += bytes([number, size, base])
    for sample in samples:
        data.append(0)
        semicircles = lambda degrees: round(degrees * 2**31 / 180)
        data += struct.pack(
            "<IiiHBBIH",
            int((sample["time"] - FIT_EPOCH).total_seconds()),
            semicircles(sample["lat"]),
            semicircles(sample["lon"]),
            round((sample["elevation"] + 500) * 5),
            sample["heart_rate"],
            sample["cadence"],
            round(sample["distance"] * 100),
            round(sample["speed"] * 1000),
        )
    if vo2_max is not None:
        # Garmin's activity metrics message, where VO2 max is in 1/65536 of a MET.
        data += bytes([0x41, 0, 0]) + struct.pack("<H", 140) + bytes([1, 7, 4, 0x86])
        data += bytes([1]) + struct.pack("<I", round(vo2_max / 3.5 * 65536))
    header = bytearray([14, 0x20]) + struct.pack("<H", 2132) + struct.pack("<I", len(data)) + b".FIT"
    header += struct.pack("<H", fit_crc(header))
    body = header + data
    return bytes(body + struct.pack("<H", fit_crc(body)))


def zipped(name, content):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(name, content)
    return buffer.getvalue()


def png(width, height, colors, rng):
    """A soft landscape-like gradient, standing in for a photo."""
    top, bottom = colors
    horizon = rng.uniform(0.45, 0.7)
    hills = rng.uniform(1.5, 3.5)
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            fraction = y / height
            wave = 0.06 * math.sin(x / width * math.pi * hills)
            color = top if fraction < horizon + wave else bottom
            shade = 0.75 + 0.25 * (1 - abs(fraction - horizon))
            rows += bytes(min(255, int(channel * shade)) for channel in color)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
            + chunk(b"IEND", b""))


# --- Data ---------------------------------------------------------------------


def pick(rng, choices):
    roll = rng.random()
    for choice in choices:
        roll -= choice[1]
        if roll <= 0:
            return choice
    return choices[-1]


def length_of(points):
    return sum(meters(a, b) for a, b in zip(points, points[1:]))


def first_part(points, limit):
    """The points up to `limit` meters from the first."""
    total = 0.0
    for index in range(1, len(points)):
        total += meters(points[index - 1], points[index])
        if total >= limit:
            return points[:index + 1]
    return points


def and_back(points):
    """There and back the same way, as one closed route."""
    return points + points[-2::-1]


def make_activities(today):
    rng = random.Random(SEED)
    route_points = {
        (city, name): points
        for city, place in PLACES.items()
        for name, points in {**place["routes"], **place["rides"]}.items()
    }

    activities = []
    files = {}
    images = {}
    next_id = 9_100_000_000
    start_day = today - timedelta(days=DAYS)
    # Fitness improves over the two years, so that the charts have a trend.
    for offset in range(DAYS + 1):
        day = start_day + timedelta(days=offset)
        progress = offset / DAYS
        for session in range(2):
            # Most days have a session, some a second one.
            if rng.random() > (0.9 if session == 0 else 0.3):
                continue
            next_id += rng.randrange(1000, 90000)
            # Mostly Berlin, with trips to the other places.
            city = "Berlin"
            if 200 <= offset < 214:
                city = "Mallorca"
            elif rng.random() < 0.12:
                city = "Hamburg"
            elif rng.random() < 0.1:
                city = "Munich"
            place = PLACES[city]
            hour = rng.choice([6, 7, 7, 8, 12, 17, 18, 18, 19])
            local = datetime.combine(day, time(hour, rng.randrange(60), rng.randrange(60)))
            started = (local - timedelta(hours=place["timezone"])).replace(tzinfo=timezone.utc)

            if rng.random() < 0.15:
                activity_type, _, minutes = pick(rng, INDOOR_TYPES)
                duration = rng.uniform(*minutes) * 60
                summary = base_summary(next_id, activity_type, typename(activity_type), local, started, duration)
                summary.update(averageHR=round(rng.uniform(110, 140)), maxHR=round(rng.uniform(150, 175)),
                               calories=round(duration / 60 * rng.uniform(7, 11)))
                if activity_type != "strength_training":
                    speed = rng.uniform(2.6, 3.2) if activity_type == "treadmill_running" else rng.uniform(6, 8)
                    summary.update(distance=round(speed * duration, 1), averageSpeed=speed)
                activities.append(summary)
                continue

            label = None
            description = None
            hard = False
            if city == "Mallorca":
                activity_type, _, speed, km = GPS_TYPES[2]
                route = "Tramuntana"
            else:
                activity_type, _, speed, km = pick(rng, GPS_TYPES)
                routes = [name for name in place["routes"] if name not in HOME_SPECIAL]
                weights = [place.get("weights", {}).get(name, 3 if name.startswith("Home") else 1) for name in routes]
                route = rng.choices(routes, weights)[0]
                if activity_type in ("trail_running", "hiking") and city == "Berlin":
                    route = "Grunewald"
                elif activity_type == "cycling" and place.get("rides") and rng.random() < 0.6:
                    route = rng.choice(list(place["rides"]))
            speed *= 0.92 + 0.12 * progress + rng.uniform(-0.05, 0.05)
            distance = rng.uniform(*km) * 1000
            points = route_points[(city, route)]
            # Runs from home and rides end at home, after whole laps.
            from_start = route.startswith("Home") or route in place["rides"]
            run = rng.random() if city == "Berlin" and activity_type == "running" else 1.0
            if run < 0.64:
                # Almost every easy run takes the same route.
                points = route_points[(city, HOME_MAIN)]
                distance = length_of(points) - 0.5
                from_start = True
            elif run < 0.78:
                # Speed work on the first part of it and back.
                main = route_points[(city, HOME_MAIN)]
                points = and_back(first_part(main, rng.uniform(0.3, 0.5) * length_of(main)))
                distance = length_of(points) - 0.5
                speed *= 1.15
                hard = True
                from_start = True
                label, description = "Intervals", rng.choice(INTERVALS)
            elif run < 0.92:
                # Long runs share the start of it and go on, to different distances.
                long_run = rng.choices(HOME_LONG, [4, 2, 2, 2])[0]
                points = route_points[(city, long_run)]
                if long_run == "Home Long East":
                    points = and_back(first_part(points[:(len(points) + 1) // 2], rng.uniform(10_000, 17_500)))
                distance = length_of(points) - 0.5
                speed *= 0.95
                from_start = True
                label, description = "Long Run", "Long and slow."
            elif from_start:
                distance = max(1, round(distance / length_of(points))) * length_of(points) - 0.5
            name = f"{city} {label or typename(activity_type)}"
            samples = track(points, distance, speed, started, place["elevation"], activity_type, progress, rng, from_start, hard)
            summary = base_summary(next_id, activity_type, name, local, started, samples[-1]["elapsed"])
            summary.update(track_summary(samples, activity_type, progress, rng))
            summary["locationName"] = city
            if description:
                summary["description"] = description
            elif rng.random() < 0.2:
                summary["description"] = rng.choice(DESCRIPTIONS)
            activities.append(summary)
            vo2_max = round(46 + 5 * progress + rng.uniform(-0.5, 0.5), 1) if activity_type in ("running", "trail_running") else None
            files[next_id] = zipped(f"{next_id}_ACTIVITY.fit", fit_file(samples, vo2_max))
            if rng.random() < 0.08:
                count = rng.randrange(1, 4)
                images[next_id] = [
                    (f"img{next_id}{index}", png(320, 220, rng.choice([
                        ((126, 172, 214), (92, 140, 82)),
                        ((240, 182, 120), (70, 92, 120)),
                        ((180, 200, 222), (210, 190, 150)),
                    ]), rng))
                    for index in range(count)
                ]
    make_races(activities, files, images)
    activities.sort(key=lambda summary: summary["startTimeGMT"], reverse=True)
    return activities, files, images


def make_races(activities, files, images):
    """Two marathons, on their courses from marathon.json and berlin_marathon.json."""
    rng = random.Random(SEED + 2)
    berlin = json.loads(Path(__file__).with_name("berlin_marathon.json").read_text())
    course = [
        [lat, lon, 38 + 5 * math.sin(position / 1500) + 2 * math.sin(position / 380)]
        for (lat, lon), position in walk(berlin, length_of(berlin) - 0.5, rng, forward=True)
    ][::12]
    race(
        activities, files, images, rng, activity_id=9_098_000_000, name="Berlin Marathon 2025", city="Berlin",
        start=datetime(2025, 9, 21, 7, 15, tzinfo=timezone.utc), seconds=14535,  # 4:02:15
        course=course, progress=0.55, vo2_max=49.0, effects=(4.4, 0.9),
        description="The wall came at 32 km. Walked a few steps, then kept going.",
    )
    race(
        activities, files, images, rng, activity_id=9_099_000_000, name="Köln Marathon 2026", city="Köln",
        start=datetime(2026, 10, 4, 7, 30, tzinfo=timezone.utc), seconds=13840,  # 3:50:40
        course=json.loads(Path(__file__).with_name("marathon.json").read_text()),
        progress=1.0, vo2_max=51.5, effects=(4.7, 1.1),
        description="Last 10 km hurt, but worth every step.",
    )


def race(activities, files, images, rng, *, activity_id, name, city, start, seconds, course, progress,
         vo2_max, effects, description):
    samples = race_samples(course, start, seconds, rng)
    summary = base_summary(activity_id, "running", name, start + timedelta(hours=2), start, seconds)
    summary.update(track_summary(samples, "running", progress, rng))
    summary.update(
        eventType={"typeKey": "race"},
        locationName=city,
        description=description,
        aerobicTrainingEffect=effects[0],
        anaerobicTrainingEffect=effects[1],
    )
    activities.append(summary)
    files[activity_id] = zipped(f"{activity_id}_ACTIVITY.fit", fit_file(samples, vo2_max))
    images[activity_id] = [
        (f"img{activity_id}{index}", png(320, 220, colors, rng))
        for index, colors in enumerate([
            ((126, 172, 214), (92, 140, 82)),
            ((240, 182, 120), (70, 92, 120)),
            ((180, 200, 222), (210, 190, 150)),
        ])
    ]


def race_samples(course, start, seconds, rng):
    """One sample per second along the course, slowing down a little towards the end."""
    total = 0.0
    distances = [0.0]
    for a, b in zip(course, course[1:]):
        total += meters(a, b)
        distances.append(total)
    # The GPS elevation is jumpy, so average it over a minute of running.
    elevations = []
    for index in range(len(course)):
        window = [point[2] for point in course[max(0, index - 30):index + 31]]
        elevations.append(sum(window) / len(window))

    def pace(distance):
        """Relative seconds per meter."""
        fraction = distance / total
        return 1 + 0.1 * fraction ** 2 + 0.025 * math.sin(distance / 2600) - (0.03 if fraction > 0.976 else 0)

    times = [0.0]
    for index in range(1, len(distances)):
        step = distances[index] - distances[index - 1]
        times.append(times[-1] + step * pace((distances[index] + distances[index - 1]) / 2))
    scale = seconds / times[-1]
    times = [moment * scale for moment in times]

    samples = []
    for second in range(seconds + 1):
        index = min(len(times) - 1, max(1, bisect.bisect_left(times, second)))
        fraction = (second - times[index - 1]) / (times[index] - times[index - 1])
        lerp = lambda values: values[index - 1] + (values[index] - values[index - 1]) * min(1.0, max(0.0, fraction))
        distance = lerp(distances)
        progress = distance / total
        warmup = min(1.0, second / 300)
        heart_rate = (104 + 44 * warmup + 26 * progress ** 1.3 + 3 * math.sin(distance / 1800)
                      + (6 if progress > 0.976 else 0) + rng.uniform(-1.5, 1.5))
        samples.append({
            "time": start + timedelta(seconds=second),
            "elapsed": float(second),
            "lat": course[index - 1][0] + (course[index][0] - course[index - 1][0]) * min(1.0, max(0.0, fraction)),
            "lon": course[index - 1][1] + (course[index][1] - course[index - 1][1]) * min(1.0, max(0.0, fraction)),
            "elevation": lerp(elevations),
            "heart_rate": max(90, min(190, round(heart_rate))),
            "cadence": round(85 - 2 * progress + rng.uniform(-2, 2)),
            "distance": distance,
            "speed": 1 / (pace(distance) * scale),
        })
    return samples


def typename(activity_type):
    return {
        "running": "Running", "trail_running": "Trail Running", "cycling": "Cycling",
        "walking": "Walking", "hiking": "Hiking", "strength_training": "Strength",
        "indoor_cycling": "Indoor Cycling", "treadmill_running": "Treadmill Running",
    }[activity_type]


def base_summary(activity_id, activity_type, name, local, started, duration):
    return {
        "activityId": activity_id,
        "activityName": name,
        "startTimeLocal": local.strftime("%Y-%m-%d %H:%M:%S"),
        "startTimeGMT": started.strftime("%Y-%m-%d %H:%M:%S"),
        "activityType": {"typeKey": activity_type},
        "eventType": {"typeKey": "training"},
        "duration": round(duration, 1),
        "elapsedDuration": round(duration, 1),
        "movingDuration": round(duration * 0.97, 1),
    }


def track(loop, distance, speed, started, base_elevation, activity_type, progress, rng, from_start=False, hard=False):
    foot = activity_type in ("running", "trail_running", "walking", "hiking")
    hilly = 4 if activity_type in ("trail_running", "hiking") else 1
    rest_hr = 52 - 6 * progress
    effort = {"running": 92, "trail_running": 100, "cycling": 80, "walking": 45, "hiking": 60}[activity_type]
    effort += 10 if hard else 0
    samples = []
    elapsed = 0.0
    last = 0.0
    phase = rng.uniform(0, math.tau)
    current_speed = speed
    for (lat, lon), position in walk(loop, distance, rng, from_start):
        # About one sample per second, as watches record.
        if position - last < current_speed and samples:
            continue
        elapsed += (position - last) / current_speed
        last = position
        current_speed = max(0.5, speed * (1 + 0.06 * math.sin(position / 900 + phase)) + rng.uniform(-0.15, 0.15))
        elevation = base_elevation + 12 * hilly * math.sin(position / 1400 + phase) + 4 * hilly * math.sin(position / 330)
        warmup = min(1.0, elapsed / 480)
        heart_rate = rest_hr + effort * (0.75 + 0.25 * warmup) + elapsed / 300 + rng.uniform(-2, 2)
        samples.append({
            "time": started + timedelta(seconds=elapsed),
            "elapsed": elapsed,
            "lat": lat,
            "lon": lon,
            "elevation": elevation,
            "heart_rate": max(60, min(200, round(heart_rate))),
            "cadence": round((84 if foot else 88) + rng.uniform(-3, 3)),
            "distance": position,
            "speed": current_speed,
        })
    return samples


def track_summary(samples, activity_type, progress, rng):
    distance = samples[-1]["distance"]
    duration = samples[-1]["elapsed"]
    rates = [sample["heart_rate"] for sample in samples]
    elevations = [sample["elevation"] for sample in samples]
    gain = sum(max(0, b - a) for a, b in zip(elevations, elevations[1:]))
    loss = sum(max(0, a - b) for a, b in zip(elevations, elevations[1:]))
    zones = [0] * 5
    for rate in rates:
        zones[min(4, max(0, (rate - 100) // 18))] += 1
    summary = {
        "distance": round(distance, 1),
        "averageSpeed": distance / duration,
        "duration": round(duration, 1),
        "elapsedDuration": round(duration, 1),
        "movingDuration": round(duration * 0.98, 1),
        "maxSpeed": max(sample["speed"] for sample in samples),
        "elevationGain": round(gain),
        "elevationLoss": round(loss),
        "minElevation": round(min(elevations)),
        "maxElevation": round(max(elevations)),
        "averageHR": round(sum(rates) / len(rates)),
        "maxHR": max(rates),
        "calories": round(duration / 60 * (11 if activity_type != "walking" else 5)),
        "startLatitude": samples[0]["lat"],
        "startLongitude": samples[0]["lon"],
        "aerobicTrainingEffect": round(rng.uniform(2.2, 4.2), 1),
        "anaerobicTrainingEffect": round(rng.uniform(0.2, 2.0), 1),
        "activityTrainingLoad": round(duration / 60 * rng.uniform(0.9, 1.6)),
        **{f"hrTimeInZone_{index + 1}": seconds for index, seconds in enumerate(zones)},
    }
    if activity_type in ("running", "trail_running"):
        summary["averageRunningCadenceInStepsPerMinute"] = 168 + round(rng.uniform(-4, 6))
        summary["maxRunningCadenceInStepsPerMinute"] = 182 + round(rng.uniform(0, 8))
        summary["steps"] = round(duration / 60 * 170)
        for split in (1000, 1609, 5000, 10000, 21098):
            if distance >= split:
                summary[f"fastestSplit_{split}"] = round(split / summary["maxSpeed"] * 1.12 + rng.uniform(0, 8), 1)
    if activity_type == "cycling":
        summary["averageBikingCadenceInRevPerMinute"] = 86 + round(rng.uniform(-4, 4))
        summary["avgPower"] = round(170 + 30 * progress + rng.uniform(-15, 15))
        summary["maxPower"] = summary["avgPower"] + round(rng.uniform(250, 450))
    return summary


def make_health(today):
    """Daily values, as Garmin's daily summaries and sleep records hold them."""
    rng = random.Random(SEED + 1)
    days = {}
    for offset in range(DAYS + 1):
        day = today - timedelta(days=DAYS - offset)
        progress = offset / DAYS
        weekday = day.weekday()
        sleep = rng.gauss(7.3 if weekday >= 5 else 6.9, 0.6)
        resting = round(56 - 6 * progress + rng.gauss(0, 1.5))
        days[day.isoformat()] = {
            "calendarDate": day.isoformat(),
            "totalSteps": round(max(2000, rng.gauss(10500 if weekday < 5 else 12500, 3000))),
            "dailyStepGoal": 10000,
            "totalDistanceMeters": None,
            "restingHeartRate": resting,
            "minHeartRate": resting - 4,
            "maxHeartRate": round(rng.uniform(140, 185)),
            "averageStressLevel": round(max(10, rng.gauss(32 - 4 * progress, 7))),
            "totalKilocalories": round(rng.gauss(2600, 300)),
            "activeKilocalories": round(rng.gauss(700, 250)),
            "floorsAscended": round(max(0, rng.gauss(11, 5))),
            "floorsDescended": round(max(0, rng.gauss(10, 5))),
            "moderateIntensityMinutes": round(max(0, rng.gauss(25, 15))),
            "vigorousIntensityMinutes": round(max(0, rng.gauss(18, 15))),
            "bodyBatteryHighestValue": round(min(100, rng.gauss(85, 8))),
            "bodyBatteryLowestValue": round(max(5, rng.gauss(22, 8))),
            "bodyBatteryChargedValue": round(rng.gauss(62, 10)),
            "bodyBatteryDrainedValue": round(rng.gauss(64, 10)),
            "averageSpo2": round(rng.gauss(96, 1)),
            "lowestSpo2": round(rng.gauss(90, 2)),
            "avgWakingRespirationValue": round(rng.gauss(14, 0.8), 1),
            "sleepTimeSeconds": round(sleep * 3600),
            "deepSleepSeconds": round(sleep * 3600 * rng.uniform(0.15, 0.22)),
            "remSleepSeconds": round(sleep * 3600 * rng.uniform(0.18, 0.25)),
            "lightSleepSeconds": round(sleep * 3600 * rng.uniform(0.5, 0.58)),
            "awakeSleepSeconds": round(rng.uniform(600, 2400)),
            "sleepScore": round(min(98, max(40, rng.gauss(78 + 4 * progress, 8)))),
            "hrv": round(rng.gauss(48 + 10 * progress, 5)),
            "weight": round((78.5 - 3 * progress + rng.gauss(0, 0.4)) * 1000),
            "hydration": round(rng.gauss(2200, 400)),
        }
    return days


def days_between(health, start, end):
    first = date.fromisoformat(start)
    last = date.fromisoformat(end)
    return [health[day.isoformat()] for day in (first + timedelta(n) for n in range((last - first).days + 1))
            if day.isoformat() in health]


def sleep_record(day):
    return {
        "calendarDate": day["calendarDate"],
        "sleepTimeSeconds": day["sleepTimeSeconds"],
        "deepSleepSeconds": day["deepSleepSeconds"],
        "lightSleepSeconds": day["lightSleepSeconds"],
        "remSleepSeconds": day["remSleepSeconds"],
        "awakeSleepSeconds": day["awakeSleepSeconds"],
        "sleepScores": {"overall": {"value": day["sleepScore"]}},
    }


def hrv_record(day):
    return {
        "calendarDate": day["calendarDate"],
        "lastNightAvg": day["hrv"],
        "weeklyAvg": day["hrv"] - 1,
        "lastNight5MinHigh": day["hrv"] + 18,
        "baseline": {"balancedLow": 42, "balancedUpper": 60},
    }


# --- Server -------------------------------------------------------------------

TODAY = date.today()
ACTIVITIES, FILES, IMAGES = make_activities(TODAY)
HEALTH = make_health(TODAY)
BY_ID = {summary["activityId"]: summary for summary in ACTIVITIES}


def answer(path, query):
    """The status and JSON or bytes for a request, like Garmin would send them."""
    one = lambda name: query.get(name, [""])[0]
    parts = path.strip("/").split("/")

    if path == "/personalapitoken-service/personal-api/personal-api-key/token":
        return 200, "demo-access-token"
    if path == "/activitylist-service/activities/search/activities":
        start, limit = int(one("start") or 0), int(one("limit") or 20)
        return 200, ACTIVITIES[start:start + limit]
    if path.startswith("/download-service/files/activity/"):
        activity_id = int(parts[-1])
        return (200, FILES[activity_id]) if activity_id in FILES else (404, {})
    if path.startswith("/activity-service/activity/"):
        activity_id = int(parts[2])
        if activity_id not in BY_ID:
            return 404, {}
        if len(parts) > 3:
            return 200, []
        images = [
            {"imageId": image_id, "url": f"{PUBLIC_BASE}/images/{activity_id}/{image_id}.png"}
            for image_id, _ in IMAGES.get(activity_id, [])
        ]
        return 200, {**BY_ID[activity_id], "metadataDTO": {"activityImages": images}}
    if path.startswith("/images/"):
        activity_id, image_id = int(parts[1]), parts[2].removesuffix(".png")
        for known, data in IMAGES.get(activity_id, []):
            if known == image_id:
                return 200, data
        return 404, {}

    if path == "/usersummary-service/v2/dailysummaries":
        return 200, days_between(HEALTH, one("startDate"), one("endDate"))
    if path == "/sleep-service/sleep/dailySleepSummariesByDate":
        return 200, [sleep_record(day) for day in days_between(HEALTH, one("startDate"), one("endDate"))]
    if path.startswith("/hrv-service/hrv/daily/"):
        return 200, {"hrvSummaries": [hrv_record(day) for day in days_between(HEALTH, parts[3], parts[4])]}
    if path == "/wellness-service/wellness/bodyBattery/reports/daily":
        return 200, [
            {"date": day["calendarDate"], "charged": day["bodyBatteryChargedValue"], "drained": day["bodyBatteryDrainedValue"]}
            for day in days_between(HEALTH, one("startDate"), one("endDate"))
        ]
    if path.startswith("/usersummary-service/stats/stress/daily/"):
        return 200, [
            {"calendarDate": day["calendarDate"], "values": {"overallStressLevel": day["averageStressLevel"]}}
            for day in days_between(HEALTH, parts[4], parts[5])
        ]
    if path.startswith("/usersummary-service/stats/steps/daily/"):
        return 200, [
            {"calendarDate": day["calendarDate"], "totalSteps": day["totalSteps"], "stepGoal": day["dailyStepGoal"]}
            for day in days_between(HEALTH, parts[4], parts[5])
        ]
    if path.startswith("/usersummary-service/stats/hydration/daily/"):
        return 200, [
            {"calendarDate": day["calendarDate"], "values": {"valueInML": day["hydration"], "goalInML": 2500}}
            for day in days_between(HEALTH, parts[4], parts[5])
        ]
    if path.startswith("/weight-service/weight/range/"):
        return 200, {"dailyWeightSummaries": [
            {"summaryDate": day["calendarDate"], "latestWeight": {"weight": day["weight"], "bmi": round(day["weight"] / 1000 / 1.82**2, 1)}}
            for day in days_between(HEALTH, parts[3], parts[4]) if date.fromisoformat(day["calendarDate"]).weekday() in (0, 3)
        ]}
    if path == "/sleep-service/sleep/dailySleep":
        day = HEALTH.get(one("date"))
        return (200, {"dailySleepDTO": sleep_record(day)}) if day else (404, {})
    if path.startswith("/hrv-service/hrv/"):
        day = HEALTH.get(parts[2])
        return (200, {"hrvSummary": hrv_record(day)}) if day else (404, {})
    if path == "/wellness-service/wellness/dailyHeartRate":
        day = HEALTH.get(one("date"))
        if not day:
            return 404, {}
        return 200, {"calendarDate": day["calendarDate"], "restingHeartRate": day["restingHeartRate"],
                     "minHeartRate": day["minHeartRate"], "maxHeartRate": day["maxHeartRate"]}
    if path.startswith("/wellness-service/wellness/daily/spo2acclimation/"):
        day = HEALTH.get(parts[-1])
        if not day:
            return 404, {}
        return 200, {"calendarDate": day["calendarDate"], "averageSpO2": day["averageSpo2"], "lowestSpO2": day["lowestSpo2"]}
    if path.startswith("/fitnessage-service/fitnessage/v2/"):
        return 200, {"fitnessAge": 31.4, "chronologicalAge": 36}
    if path == "/userprofile-service/userprofile/settings":
        return 200, {"userData": {"gender": "FEMALE", "measurementSystem": "metric"}}
    if path == "/biometric-service/heartRateZones/v2":
        return 200, [{"sport": "DEFAULT", "zone1Floor": 100, "zone2Floor": 118, "zone3Floor": 136,
                      "zone4Floor": 154, "zone5Floor": 172, "maxHeartRateUsed": 190}]
    if path.startswith("/wellnessactivity-service/") or path.startswith("/goal-service/"):
        return 200, []
    # Food logs and the menstrual cycle: Garmin refuses those for most accounts.
    return 403, {}


class Handler(BaseHTTPRequestHandler):
    def respond(self):
        url = urlparse(self.path)
        status, body = answer(url.path, parse_qs(url.query))
        if isinstance(body, bytes):
            data = body
            kind = "image/png" if url.path.startswith("/images/") else "application/zip"
        elif isinstance(body, str):
            data, kind = body.encode(), "text/plain"
        else:
            data, kind = json.dumps(body).encode(), "application/json"
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = respond
    do_POST = respond

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print(f"{len(ACTIVITIES)} activities, {len(FILES)} with GPS, {len(HEALTH)} days of health data", flush=True)
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
