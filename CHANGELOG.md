# Update-Log

## 1.1.0

### Neu
- **Seitengröße-Regler** oben rechts – Doppelklick setzt auf 100 %, Mausrad funktioniert auch. Wird pro Website gemerkt (wie Strg+Mausrad).
- **Tabs in der Farbe der Website** – YouTube rot, SoundCloud orange usw., statt immer grün.
- **Popup-Fenster** – Logins mit Google, Steam, Discord & Co. öffnen sich als echtes Popup und funktionieren dadurch. Ungefragte Werbe-Popups werden geblockt.
- **Externe Apps** – Links wie `steam://` öffnen direkt die App: CS2-Server beitreten (z. B. über xplay.gg), Spiele starten, Discord, Spotify, Epic, Battle.net …
- **Download-Anzeige** – Fortschrittsring mit Dateiname und Prozent in der Leiste, danach „Fertig“. Das Download-Panel öffnet sich automatisch.
- **Viel stärkerer Werbeblocker** – über 3 600 Werbe- und Tracking-Server (inkl. der Liste von Peter Lowe), dazu gezielte Filter:
  - **YouTube:** Werbung wird schon aus den Video-Daten entfernt, bevor der Player sie sieht; was trotzdem kommt, wird stumm vorgespult und übersprungen.
  - **SoundCloud:** Audio-Werbung (AdsWizz) wird blockiert.
  - Audio- und Video-Werbenetzwerke (FreeWheel, SpotX, Innovid …) auf allen Seiten.

### Behoben
- **YouTube stoppte beim Tab-Wechsel** – Hintergrund-Tabs werden nicht mehr sofort eingefroren. Musik und Videos laufen weiter.
- **Tabs luden beim Zurückwechseln neu** – Tabs werden erst nach 5 min im Hintergrund pausiert und nach 30 min entladen, nie solange sie Ton abspielen.
- **Manche Seiten zeigten die Fehlerseite** – Leaf zeigt sie nur noch bei echten Verbindungsfehlern. Antwortet der Server (auch mit 403/404), erscheint dessen Seite.
- **Downloads wurden nicht angezeigt** – ein Klick auf einen Download-Link ersetzte die Seite durch die Fehlerseite. Jetzt startet der Download sichtbar und die Seite bleibt stehen.
- **Zufällige Abstürze** – eine Absturzquelle beim Tab-Wechsel beseitigt. Stürzt die Web-Engine selbst ab, startet Leaf sich mit allen Tabs neu. Seiten-Abstürze laden den Tab neu.
- **Seiten laden schneller** – der Werbeblocker prüft nur noch Anfragen an Werbeserver, statt jede Anfrage über Leaf zu leiten. Prozess-Limits entfernt, die Seiten ausbremsten.

## 1.0.0

Erste Version: Liquid-Glass-Design, Tabs, Verlauf, Favoriten, Werbeblocker, Downloads, Sitzung wiederherstellen, angemeldet bleiben, Passwörter, Standardbrowser.
