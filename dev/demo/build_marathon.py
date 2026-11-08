"""The courses of the made-up marathons, from the GPX files next to this script.

Run once with `python3 dev/demo/build_marathon.py` to write marathon.json and
berlin_marathon.json, which fake_garmin.py reads. Only the positions are kept,
and the elevation of Köln. Pace, heart rate and cadence are made up there, so
the GPX files themselves are not committed.
"""

import json
import xml.etree.ElementTree as ET
from pathlib import Path

NS = {"gpx": "http://www.topografix.com/GPX/1/1"}
# GPX file, JSON file, and whether the elevation is kept.
COURSES = [
    ("Köln Marathon.gpx", "marathon.json", True),
    ("BM25_Marathon-Strecke.gpx", "berlin_marathon.json", False),
]


def main():
    folder = Path(__file__).parent
    for gpx, target, elevation in COURSES:
        if not (folder / gpx).exists():
            continue
        root = ET.parse(folder / gpx).getroot()
        points = []
        for point in root.iterfind(".//gpx:trkpt", NS):
            position = [round(float(point.get("lat")), 6), round(float(point.get("lon")), 6)]
            if elevation:
                position.append(round(float(point.find("gpx:ele", NS).text), 1))
            points.append(position)
        (folder / target).write_text(json.dumps(points, separators=(",", ":")) + "\n")
        print(target, len(points), "points")


main()
