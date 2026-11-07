"""A stand-in for the Garmin Personal API that serves made-up data.

Runmap syncs from it like from Garmin, so the demo shows every page without
anyone's real activities. Everything comes from a fixed seed, so the data is
the same on every start, apart from moving along with today's date.

Only the Python standard library, so that it runs in a plain python image.
"""

import io
import json
import math
import random
import struct
import zipfile
import zlib
from datetime import date, datetime, time, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

SEED = 7
DAYS = 730
PORT = 8000
# The address the backend reaches this server at, for the photo links.
PUBLIC_BASE = "http://fake-garmin:8000"

# Routes as waypoints, as (latitude, longitude). Loops end where they start.
PLACES = {
    "Berlin": {
        "timezone": 1,
        "elevation": 40,
        "routes": {
            "Tiergarten": [
                (52.5145, 13.3501), (52.5163, 13.3640), (52.5162, 13.3770), (52.5122, 13.3770),
                (52.5095, 13.3650), (52.5098, 13.3480), (52.5120, 13.3390), (52.5145, 13.3501),
            ],
            "Tempelhofer Feld": [
                (52.4805, 13.3905), (52.4810, 13.4040), (52.4770, 13.4170), (52.4690, 13.4150),
                (52.4675, 13.4010), (52.4720, 13.3920), (52.4805, 13.3905),
            ],
            "Spree": [
                (52.5190, 13.3700), (52.5205, 13.3900), (52.5170, 13.4080), (52.5110, 13.4200),
                (52.5030, 13.4380), (52.4990, 13.4470), (52.5040, 13.4400), (52.5120, 13.4230),
                (52.5180, 13.4090), (52.5212, 13.3880), (52.5190, 13.3700),
            ],
            "Grunewald": [
                (52.4890, 13.2610), (52.4800, 13.2480), (52.4660, 13.2420), (52.4560, 13.2310),
                (52.4480, 13.2450), (52.4600, 13.2620), (52.4760, 13.2700), (52.4890, 13.2610),
            ],
        },
    },
    "Hamburg": {
        "timezone": 1,
        "elevation": 10,
        "routes": {
            "Alster": [
                (53.5560, 10.0010), (53.5620, 9.9970), (53.5700, 9.9990), (53.5770, 10.0040),
                (53.5760, 10.0130), (53.5680, 10.0150), (53.5600, 10.0100), (53.5560, 10.0010),
            ],
            "Stadtpark": [
                (53.5960, 10.0190), (53.5990, 10.0300), (53.5960, 10.0420), (53.5900, 10.0360),
                (53.5910, 10.0230), (53.5960, 10.0190),
            ],
        },
    },
    "Munich": {
        "timezone": 1,
        "elevation": 520,
        "routes": {
            "Englischer Garten": [
                (48.1450, 11.5880), (48.1530, 11.5930), (48.1620, 11.5960), (48.1720, 11.6020),
                (48.1680, 11.6090), (48.1570, 11.6020), (48.1480, 11.5960), (48.1450, 11.5880),
            ],
            "Isar": [
                (48.1300, 11.5780), (48.1220, 11.5700), (48.1100, 11.5600), (48.0980, 11.5480),
                (48.1060, 11.5560), (48.1180, 11.5680), (48.1300, 11.5780),
            ],
        },
    },
    "Mallorca": {
        "timezone": 1,
        "elevation": 120,
        "routes": {
            "Tramuntana": [
                (39.6950, 2.7900), (39.7300, 2.7200), (39.7700, 2.7100), (39.7900, 2.6900),
                (39.7600, 2.6500), (39.7100, 2.6700), (39.6950, 2.7900),
            ],
        },
    },
}

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


def catmull_rom(points, steps=24):
    """A smooth closed curve through the waypoints."""
    smooth = []
    count = len(points) - 1
    for index in range(count):
        p0, p1, p2, p3 = (points[(index + offset) % count] for offset in (-1, 0, 1, 2))
        for step in range(steps):
            t = step / steps
            smooth.append(tuple(
                0.5 * (2 * p1[axis] + (-p0[axis] + p2[axis]) * t
                       + (2 * p0[axis] - 5 * p1[axis] + 4 * p2[axis] - p3[axis]) * t * t
                       + (-p0[axis] + 3 * p1[axis] - 3 * p2[axis] + p3[axis]) * t ** 3)
                for axis in (0, 1)
            ))
    return smooth


def meters(a, b):
    lat = math.radians((a[0] + b[0]) / 2)
    dy = (b[0] - a[0]) * 111_320
    dx = (b[1] - a[1]) * 111_320 * math.cos(lat)
    return math.hypot(dx, dy)


def walk(loop, distance, rng):
    """Positions every meter along `loop`, round and round, for `distance` meters."""
    offset_lat = rng.uniform(-0.00008, 0.00008)
    offset_lon = rng.uniform(-0.00012, 0.00012)
    reverse = rng.random() < 0.5
    loop = list(reversed(loop)) if reverse else loop
    start = rng.randrange(len(loop))
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
        yield (current[0] + offset_lat, current[1] + offset_lon), position
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


def fit_file(samples):
    """A FIT file with one record message per sample."""
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


def make_activities(today):
    rng = random.Random(SEED)
    loops = {
        (city, name): catmull_rom(points)
        for city, place in PLACES.items()
        for name, points in place["routes"].items()
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
        if rng.random() > 0.42:
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

        if city == "Mallorca":
            activity_type, _, speed, km = GPS_TYPES[2]
            route = "Tramuntana"
        else:
            activity_type, _, speed, km = pick(rng, GPS_TYPES)
            routes = list(place["routes"])
            route = "Grunewald" if activity_type in ("trail_running", "hiking") and city == "Berlin" else rng.choice(routes)
        speed *= 0.92 + 0.12 * progress + rng.uniform(-0.05, 0.05)
        distance = rng.uniform(*km) * 1000
        name = f"{city} {typename(activity_type)}"
        samples = track(loops[(city, route)], distance, speed, started, place["elevation"], activity_type, progress, rng)
        summary = base_summary(next_id, activity_type, name, local, started, samples[-1]["elapsed"])
        summary.update(track_summary(samples, activity_type, progress, rng))
        summary["locationName"] = city
        if rng.random() < 0.2:
            summary["description"] = rng.choice(DESCRIPTIONS)
        if activity_type in ("running", "trail_running") and rng.random() < 0.04:
            summary["eventType"] = {"typeKey": "race"}
            summary["activityName"] = f"{city} City Run"
        activities.append(summary)
        files[next_id] = zipped(f"{next_id}_ACTIVITY.fit", fit_file(samples))
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
    activities.sort(key=lambda summary: summary["startTimeGMT"], reverse=True)
    return activities, files, images


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


def track(loop, distance, speed, started, base_elevation, activity_type, progress, rng):
    foot = activity_type in ("running", "trail_running", "walking", "hiking")
    hilly = 4 if activity_type in ("trail_running", "hiking") else 1
    rest_hr = 52 - 6 * progress
    effort = {"running": 92, "trail_running": 100, "cycling": 80, "walking": 45, "hiking": 60}[activity_type]
    samples = []
    elapsed = 0.0
    last = 0.0
    phase = rng.uniform(0, math.tau)
    current_speed = speed
    for (lat, lon), position in walk(loop, distance, rng):
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
        summary["vO2MaxValue"] = round(46 + 5 * progress + rng.uniform(-0.5, 0.5))
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
