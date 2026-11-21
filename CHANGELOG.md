# Update-Log

## 1.1.2

### Schneller
- **Neue Tabs öffnen schneller** – Leaf hält im Leerlauf einen unsichtbaren, fertig vorbereiteten Tab bereit. Ein neuer Tab oder ein Tab aus der letzten Sitzung muss dadurch nicht erst erzeugt werden.
  Gemessen (Link öffnen bis Seite geladen, je 4×): **599 ms statt 641 ms** im Median, alle Werte mit Reserve-Tab schneller als alle ohne. Kostet ca. 15–20 MB RAM.

### Geprüft
- **YouTube-Werbung, live:** drei beliebte Musikvideos (Despacito, Shape of You, Bohemian Rhapsody) – ohne Blocker lief bei allen dreien Werbung, **mit Blocker bei keinem**, und die Videos spielten normal.
- **Belastungstest gegen Abstürze:** 2 000 zufällige Aktionen in 2½ Minuten (Tabs öffnen, schließen, wechseln, verschieben, navigieren, zoomen …), dabei rund 580 Tabs erzeugt und wieder zerstört – kein Absturz, kein Hänger, sauber beendet.

### Für Entwickler
Testhilfen (nur mit `LEAF_DEBUG=1`): eigenes Profil `LEAF_PROFILE`, Belastungstest `LEAF_STRESS=<Anzahl>`, Ereignis-Log mit Zeitstempeln in `debug.log`.

## 1.1.1

### Behoben
- **Login-Popups blieben nach dem Schließen von Leaf offen** und hielten die Web-Engine im Hintergrund am Laufen – sie werden jetzt mit Leaf geschlossen.
- **Steam-/App-Links ersetzten die Seite durch die Fehlerseite** (z. B. „Server beitreten“ auf xplay.gg) – die Seite bleibt jetzt stehen, Steam startet.
- **Geblockte Werbe-Skripte** (Google Ads, AdSense, Analytics, Tag Manager) werden durch harmlose Attrappen ersetzt statt mit einem Fehler abgebrochen – so warten Seiten nicht auf Timeouts und Links mit Tracking funktionieren weiter.
- Seiten ohne normale Adresse (z. B. `data:`) wurden fälschlich als leerer Tab behandelt.

### Korrektur zu 1.1.0
Die Aussage „Seiten laden schneller“ war zu optimistisch: Nachgemessen (Spiegel, Chip, GitHub, Wikipedia, je dreimal mit leerem Cache) laden einzelne Seiten **nicht messbar schneller** als vorher. Spürbar schneller ist der Alltag trotzdem, weil Tabs beim Zurückwechseln nicht mehr neu laden.

### Geprüft
- YouTube läuft im Hintergrund-Tab weiter (35 s beobachtet, echtes YouTube-Video)
- YouTube-Werbedaten werden auf echten YouTube-Seiten entfernt
- Login-Popup → eigenes Fenster, `target=_blank` → Tab, ungefragtes Popup → geblockt
- `steam://connect/…` wird an Steam übergeben, die Seite bleibt stehen
- Absturz der Web-Engine → Leaf startet sich mit allen Tabs neu

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
- **Weniger Last** – der Werbeblocker prüft nur noch Anfragen an Werbeserver, statt jede Anfrage über Leaf zu leiten; Prozess-Limit entfernt (siehe Korrektur in 1.1.1).

## 1.0.0

Erste Version: Liquid-Glass-Design, Tabs, Verlauf, Favoriten, Werbeblocker, Downloads, Sitzung wiederherstellen, angemeldet bleiben, Passwörter, Standardbrowser.
