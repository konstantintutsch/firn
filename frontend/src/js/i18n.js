/**
 * Translations. The English text is the key, so that a missing translation
 * falls back to English. German is used if the browser prefers it to English.
 */

const preferred = (navigator.languages ?? [navigator.language]).find((tag) => /^(de|en)\b/i.test(tag));
export const language = preferred?.toLowerCase().startsWith("de") ? "de" : "en";

const GERMAN = {
  // Pages and navigation
  "Runmap statistics": "Runmap-Statistik",
  "Runmap activities": "Runmap-Aktivitäten",
  "Runmap settings": "Runmap-Einstellungen",
  Map: "Karte",
  Activities: "Aktivitäten",
  Statistics: "Statistik",
  Settings: "Einstellungen",
  Pages: "Seiten",
  Sync: "Synchronisieren",
  "Switch to light mode": "Zum hellen Design wechseln",
  "Switch to dark mode": "Zum dunklen Design wechseln",

  // Sign-in and settings
  "Your Garmin activities on a map, with statistics on training, sleep and health.":
    "Deine Garmin-Aktivitäten auf einer Karte, mit Statistiken zu Training, Schlaf und Gesundheit.",
  "Sign in": "Anmelden",
  "Sign out": "Abmelden",
  "Garmin Personal API key": "Persönlicher Garmin-API-Schlüssel",
  "Create one on": "Erstelle einen bei",
  "Runmap stores it encrypted and never shows it again.":
    "Runmap speichert ihn verschlüsselt und zeigt ihn nie wieder an.",
  "Save key": "Schlüssel speichern",
  "Remove key": "Schlüssel entfernen",
  "Signed in": "Angemeldet",
  "A Garmin key is saved.": "Ein Garmin-Schlüssel ist gespeichert.",
  "No Garmin key is saved yet. Add one to sync your data.":
    "Noch kein Garmin-Schlüssel gespeichert. Füge einen hinzu, um deine Daten zu synchronisieren.",
  "Checking the key with Garmin…": "Schlüssel wird bei Garmin geprüft…",
  "Saved. Select Sync to save your data.": "Gespeichert. Wähle Synchronisieren, um deine Daten zu speichern.",
  "Could not save the key.": "Der Schlüssel konnte nicht gespeichert werden.",
  "Removed. Your saved data stays.": "Entfernt. Deine gespeicherten Daten bleiben erhalten.",
  "Could not remove the key.": "Der Schlüssel konnte nicht entfernt werden.",
  "Could not reach the API. Is it running?": "Die API ist nicht erreichbar. Läuft sie?",
  "Signing in did not work. Try again.": "Die Anmeldung hat nicht geklappt. Versuche es noch einmal.",
  "Could not load your account.": "Dein Konto konnte nicht geladen werden.",

  // Sync
  "Add your Garmin key first.": "Füge zuerst deinen Garmin-Schlüssel hinzu.",
  "Starting…": "Wird gestartet…",
  "Could not start the sync.": "Die Synchronisierung konnte nicht gestartet werden.",
  "Could not start the sync. Is the API running?":
    "Die Synchronisierung konnte nicht gestartet werden. Läuft die API?",
  "Saving health data {current} of {total}…": "Gesundheitsdaten {current} von {total} werden gespeichert…",
  "Looking for new activities…": "Neue Aktivitäten werden gesucht…",
  "Saving activity {current} of {total}…": "Aktivität {current} von {total} wird gespeichert…",
  "No new activities.": "Keine neuen Aktivitäten.",
  "Saved {count}.": "{count} gespeichert.",
  "{saved} Health data is up to date.": "{saved} Die Gesundheitsdaten sind aktuell.",
  "{saved} {problems} failed. Sync again to retry.":
    "{saved} {problems} fehlgeschlagen. Synchronisiere erneut, um es noch einmal zu versuchen.",
  and: "und",
  activity: "Aktivität",
  activities: "Aktivitäten",
  "new activity": "neue Aktivität",
  "new activities": "neue Aktivitäten",
  "health data request": "Abfrage von Gesundheitsdaten",
  "health data requests": "Abfragen von Gesundheitsdaten",

  // Errors from the API
  "a sync is already running": "Eine Synchronisierung läuft bereits.",
  "from is after to": "Das Startdatum liegt nach dem Enddatum.",
  "Garmin refused this API key": "Garmin hat diesen API-Schlüssel abgelehnt.",
  "sign in first": "Melde dich zuerst an.",
  "the API key is empty": "Der API-Schlüssel ist leer.",

  // Map
  "Heatmap of all activities": "Heatmap aller Aktivitäten",
  "Search places and activities": "Orte und Aktivitäten suchen",
  "Show all": "Alle zeigen",
  Filters: "Filter",
  "Activity type": "Aktivitätsart",
  Title: "Titel",
  Description: "Beschreibung",
  Location: "Ort",
  "Min. km": "Min. km",
  "Max. km": "Max. km",
  "Min. minutes": "Min. Minuten",
  "Max. minutes": "Max. Minuten",
  From: "Von",
  To: "Bis",
  "Clear filters": "Filter zurücksetzen",
  "Fewer activities": "Weniger Aktivitäten",
  "More activities": "Mehr Aktivitäten",
  Places: "Orte",
  "Nothing found.": "Nichts gefunden.",
  "Could not search. Is the API running?": "Die Suche hat nicht geklappt. Läuft die API?",
  "Could not load activities. Is the API running?": "Die Aktivitäten konnten nicht geladen werden. Läuft die API?",
  "No activities match these filters.": "Keine Aktivitäten passen zu diesen Filtern.",
  "No activities saved yet. Select Sync to save them from Garmin.":
    "Noch keine Aktivitäten gespeichert. Wähle Synchronisieren, um sie von Garmin zu holen.",
  "Could not find this activity.": "Diese Aktivität wurde nicht gefunden.",
  "Find my location": "Meinen Standort finden",
  "Location not available": "Standort nicht verfügbar",
  "Zoom in": "Vergrößern",
  "Zoom out": "Verkleinern",
  "Toggle attribution": "Quellenangaben ein- oder ausblenden",

  // Selected activity
  "Show all activities": "Alle Aktivitäten zeigen",
  Profile: "Profil",
  "Track color": "Farbe des Tracks",
  Heatmap: "Heatmap",
  "Fastest splits": "Schnellste Abschnitte",
  "Heart rate zones": "Herzfrequenzbereiche",
  "Zone {zone}": "Bereich {zone}",
  "Move along the profile or the track to see the values there.":
    "Fahre über das Profil oder den Track, um die Werte dort zu sehen.",
  "Moving time": "Bewegungszeit",
  "Elapsed time": "Verstrichene Zeit",
  "Average pace": "Durchschnittliches Tempo",
  "Average speed": "Durchschnittsgeschwindigkeit",
  "Grade adjusted pace": "Steigungsbereinigtes Tempo",
  "Best pace": "Bestes Tempo",
  "Max. speed": "Max. Geschwindigkeit",
  "Elevation loss": "Höhenmeter bergab",
  "Lowest point": "Tiefster Punkt",
  "Highest point": "Höchster Punkt",
  "Average heart rate": "Durchschnittliche Herzfrequenz",
  "Max. heart rate": "Max. Herzfrequenz",
  "Average cadence": "Durchschnittliche Schrittfrequenz",
  "Max. cadence": "Max. Schrittfrequenz",
  "Average power": "Durchschnittliche Leistung",
  "Normalized power": "Normalisierte Leistung",
  "Max. power": "Max. Leistung",
  "Stride length": "Schrittlänge",
  "Vertical oscillation": "Vertikale Bewegung",
  "Vertical ratio": "Vertikales Verhältnis",
  "Ground contact time": "Bodenkontaktzeit",
  "Aerobic training effect": "Aerober Trainingseffekt",
  "Anaerobic training effect": "Anaerober Trainingseffekt",
  "Average respiration": "Durchschnittliche Atemfrequenz",
  "Moderate intensity minutes": "Moderate Intensitätsminuten",
  "Vigorous intensity minutes": "Hohe Intensitätsminuten",
  "Estimated sweat loss": "Geschätzter Schweißverlust",
  "Lowest temperature": "Niedrigste Temperatur",
  "Highest temperature": "Höchste Temperatur",
  Laps: "Runden",
  "1 mile": "1 Meile",
  "Half marathon": "Halbmarathon",
  Marathon: "Marathon",
  Speed: "Geschwindigkeit",
  Elevation: "Höhe",
  "Heart rate": "Herzfrequenz",
  Cadence: "Kadenz",
  Power: "Leistung",
  Temperature: "Temperatur",
  spm: "Schritte/min",
  rpm: "U/min",

  // Activity list
  "Search activities": "Aktivitäten suchen",
  "Load more": "Mehr laden",
  "Loading…": "Wird geladen…",
  "No GPS track": "Kein GPS-Track",

  // Statistics
  Period: "Zeitraum",
  "30 days": "30 Tage",
  "90 days": "90 Tage",
  "1 year": "1 Jahr",
  All: "Alles",
  Health: "Gesundheit",
  "Could not load statistics. Is the API running?": "Die Statistik konnte nicht geladen werden. Läuft die API?",
  "Nothing saved for this period. Select Sync to save your data from Garmin.":
    "Für diesen Zeitraum ist nichts gespeichert. Wähle Synchronisieren, um deine Daten von Garmin zu holen.",
  day: "Tag",
  week: "Woche",
  month: "Monat",
  "Week of {day}, {year}": "Woche ab {day} {year}",
  "Per {size}": "Pro {size}",
  "Total per {size}": "Summe pro {size}",
  "Per {size}, by activity type": "Pro {size}, nach Aktivitätsart",
  "{subtitle}, averaged per {size}": "{subtitle}, gemittelt pro {size}",
  "Averaged per {size}": "Gemittelt pro {size}",
  Distance: "Distanz",
  Time: "Zeit",
  "Elevation gain": "Höhenmeter",
  Calories: "Kalorien",
  Other: "Sonstige",
  "Distance per period by activity type": "Distanz pro Zeitraum nach Aktivitätsart",
  "Training time": "Trainingszeit",
  "Training time per period": "Trainingszeit pro Zeitraum",
  "Time by activity type": "Zeit nach Aktivitätsart",
  "With the number of activities": "Mit der Anzahl der Aktivitäten",
  "Running pace": "Lauftempo",
  "Average per run, lower is faster": "Durchschnitt pro Lauf, weniger ist schneller",
  Pace: "Tempo",
  "Running pace over time": "Lauftempo im Zeitverlauf",
  "Heart rate in activities": "Herzfrequenz bei Aktivitäten",
  "Average per activity": "Durchschnitt pro Aktivität",
  "VO2 max": "VO2max",
  "As Garmin estimated it after activities": "Wie Garmin sie nach Aktivitäten geschätzt hat",
  "Training load": "Trainingsbelastung",
  Load: "Belastung",
  "Training load per period": "Trainingsbelastung pro Zeitraum",
  "{label}, average": "{label}, Durchschnitt",
  "Resting heart rate": "Ruhepuls",
  HRV: "HFV",
  Sleep: "Schlaf",
  "Sleep score": "Schlafwert",
  Steps: "Schritte",
  Stress: "Stress",
  "Per night, by stage": "Pro Nacht, nach Schlafphase",
  Deep: "Tief",
  Light: "Leicht",
  REM: "REM",
  Awake: "Wach",
  Score: "Wert",
  "Heart rate variability": "Herzfrequenzvariabilität",
  "Overnight average, in ms": "Durchschnitt über Nacht, in ms",
  "Last night": "Letzte Nacht",
  "7-day average": "7-Tage-Durchschnitt",
  Resting: "In Ruhe",
  "Daily average, from 0 to 100": "Tagesdurchschnitt, von 0 bis 100",
  "Body Battery": "Body Battery",
  Highest: "Höchster",
  Lowest: "Niedrigster",
  "Per day": "Pro Tag",
  "Intensity minutes": "Intensitätsminuten",
  "Total per period": "Summe pro Zeitraum",
  Moderate: "Moderat",
  Vigorous: "Hoch",
  "Floors climbed": "Gestiegene Etagen",
  Floors: "Etagen",
  Total: "Gesamt",
  Weight: "Gewicht",
  "Body fat": "Körperfett",
  "Blood oxygen": "Blutsauerstoff",
  Average: "Durchschnitt",
  Respiration: "Atmung",
  "Breaths per minute while awake": "Atemzüge pro Minute im Wachzustand",
  brpm: "Atemzüge/min",
  Hydration: "Flüssigkeitszufuhr",
  Water: "Wasser",
  "Fitness age": "Fitnessalter",
  Age: "Alter",
  years: "Jahre",
  "Show values": "Werte zeigen",
  Day: "Tag",
};

/** Garmin's activity types, for those that read differently in German than in English. */
const GERMAN_TYPES = {
  running: "Laufen",
  trail_running: "Traillauf",
  treadmill_running: "Laufband",
  track_running: "Bahnlauf",
  indoor_running: "Indoor-Laufen",
  street_running: "Straßenlauf",
  virtual_run: "Virtueller Lauf",
  walking: "Gehen",
  casual_walking: "Spazierengehen",
  speed_walking: "Walking",
  hiking: "Wandern",
  mountaineering: "Bergsteigen",
  cycling: "Radfahren",
  road_biking: "Rennrad",
  mountain_biking: "Mountainbike",
  gravel_cycling: "Gravel",
  cyclocross: "Cyclocross",
  indoor_cycling: "Indoor-Radfahren",
  virtual_ride: "Virtuelle Radfahrt",
  e_bike_fitness: "E-Bike",
  e_bike_mountain: "E-Mountainbike",
  bmx: "BMX",
  swimming: "Schwimmen",
  lap_swimming: "Bahnenschwimmen",
  open_water_swimming: "Freiwasserschwimmen",
  multi_sport: "Multisport",
  triathlon: "Triathlon",
  strength_training: "Krafttraining",
  cardio: "Cardio",
  elliptical: "Crosstrainer",
  stair_climbing: "Treppensteigen",
  indoor_rowing: "Indoor-Rudern",
  rowing: "Rudern",
  kayaking: "Kajak",
  paddling: "Paddeln",
  stand_up_paddleboarding: "Stand-up-Paddling",
  sailing: "Segeln",
  surfing: "Surfen",
  inline_skating: "Inlineskaten",
  skating: "Eislaufen",
  resort_skiing_snowboarding: "Skifahren und Snowboarden",
  resort_skiing: "Skifahren",
  backcountry_skiing: "Skitour",
  cross_country_skiing: "Langlauf",
  skate_skiing: "Skating-Langlauf",
  snowshoeing: "Schneeschuhwandern",
  rock_climbing: "Klettern",
  bouldering: "Bouldern",
  golf: "Golf",
  tennis: "Tennis",
  yoga: "Yoga",
  pilates: "Pilates",
  other: "Sonstiges",
};

/** Translates `text` and fills in `{name}` placeholders from `values`. */
export function t(text, values = {}) {
  const template = language === "de" ? (GERMAN[text] ?? text) : text;
  return template.replace(/\{(\w+)\}/g, (match, name) => (name in values ? values[name] : match));
}

/** The count with the singular or plural English noun, translated. */
export function plural(count, one, other) {
  return `${count.toLocaleString()} ${t(count === 1 ? one : other)}`;
}

export function typeName(type) {
  if (language === "de" && type in GERMAN_TYPES) return GERMAN_TYPES[type];
  const words = type.replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * Translates the page: the text of each element with `data-i18n`, and the
 * attributes that `data-i18n-attrs` lists, separated by spaces.
 */
function translatePage() {
  document.documentElement.lang = language;
  document.title = t(document.title);
  if (language === "en") return;
  for (const element of document.querySelectorAll("[data-i18n]")) {
    element.textContent = t(element.textContent.trim());
  }
  for (const element of document.querySelectorAll("[data-i18n-attrs]")) {
    for (const name of element.dataset.i18nAttrs.split(" ")) {
      const value = element.getAttribute(name);
      if (value) element.setAttribute(name, t(value));
    }
  }
}

translatePage();
