"""The course of the made-up Köln Marathon, from the GPX file next to this script.

Run once with `python3 dev/demo/build_marathon.py` to write marathon.json, which
fake_garmin.py reads. Only the positions and elevations are kept. Pace, heart
rate and cadence are made up there, so the GPX file itself is not committed.
"""

import json
import xml.etree.ElementTree as ET
from pathlib import Path

NS = {"gpx": "http://www.topografix.com/GPX/1/1"}


def main():
    folder = Path(__file__).parent
    root = ET.parse(folder / "Köln Marathon.gpx").getroot()
    points = [
        [round(float(point.get("lat")), 6), round(float(point.get("lon")), 6),
         round(float(point.find("gpx:ele", NS).text), 1)]
        for point in root.iterfind(".//gpx:trkpt", NS)
    ]
    (folder / "marathon.json").write_text(json.dumps(points, separators=(",", ":")) + "\n")
    print(len(points), "points")


main()
