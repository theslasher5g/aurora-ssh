Aurora demo GIFs and theme screenshots - how they were recorded
=================================================================

Everything here is driven by xdotool against a real Aurora build running on a
private Xvfb display; ffmpeg (x11grab) records, palettegen/paletteuse make the GIFs
(12/10/8 fps, <= 960 px wide, looping, each <= 2.5 MB). No source code is involved.

Files
  connect.gif    Ctrl+K palette -> SSH session on aurora-lab -> ls / coloured output
  split.gif      four sessions, placed side by side (rail chip -> "Place next to current session")
  inventory.gif  customer -> site -> host tree, expand/collapse, "only this customer" focus,
                 search language (type:ssh, type:rdp, env:prod, #postgres)
  logs.gif       SSH session + docked log panel (Ctrl+Shift+L) following a growing file, filters
  themes.gif     same scene cycling through 7 themes via Ctrl+K "Theme: <name>"
  ../themes/<id>.png  one PNG per theme id (26), same scene, 960 px wide, 128 colours
  scripts/       the drivers (see below)

Requirements: ffmpeg, ImageMagick (convert/import/montage), xdotool, Xvfb, python3,
an sshd on 127.0.0.1:2222 (user aurora / password testpass123) and a release or debug build
of aurora-term (cargo build; copy target/debug/aurora-term to /tmp/demo/aurora).
The scripts assume root on the test machine (they write /home/aurora/demo and .bashrc).

Re-recording (all scripts: docs/demo/scripts/, run from the repo root)
  0. Xvfb :81 -screen 0 1600x1000x24 &                       # the scripts use DISPLAY=:81
     mkdir -p /tmp/demo; cp -r <test data with vault> /tmp/demo/data
     cp target/debug/aurora-term /tmp/demo/aurora
     (the vault password is passed in PW=..., default is the test vault's)
     The aurora-lab host needs a saved keychain entry once (connect once, tick "Save in keychain").
  1. bash docs/demo/scripts/prep-lab.sh        # clean ~/demo files, prompt per host, log writer script
  2. bash docs/demo/scripts/reset.sh [THEME]   # rebuilds the demo inventory (mkdata.py) and restarts + unlocks the app
  3. one of:
       bash docs/demo/scripts/connect.sh       -> /tmp/demo/rec/connect.gif
       bash docs/demo/scripts/split.sh         -> /tmp/demo/rec/split.gif
       bash docs/demo/scripts/inventory.sh     -> /tmp/demo/rec/inventory.gif
       bash docs/demo/scripts/logwriter.sh start; bash docs/demo/scripts/logs.sh   -> logs.gif
       bash docs/demo/scripts/logwriter.sh start; bash docs/demo/scripts/themes-gif.sh -> themes.gif
       bash docs/demo/scripts/logwriter.sh start; bash docs/demo/scripts/themes.sh docs/themes  -> 26 PNGs
     Run reset.sh before EVERY scene (each scene starts from a fresh, unlocked app).
  4. bash docs/demo/scripts/logwriter.sh stop; kill the app (PID in /tmp/demo/app.pid)
     Restore the test account: cp /home/aurora/.bashrc.demo-bak /home/aurora/.bashrc

Scripts
  lib.sh          helpers: mv (smooth mouse move), click, typ, run, rec_start/rec_stop, to_gif (SPEED, DITHER env)
  regif.sh        re-encode an existing /tmp/demo/rec/NAME.mkv:  SPEED=1.3 regif.sh NAME FPS WIDTH COLORS TRIM
  mkdata.py       writes the demo inventory into a COPY of library.json (3 customers with sites, PROD/STAGING
                  labels, tags, RDP/Telnet/VNC hosts, English settings); every host points at 127.0.0.N:2222
                  and uses the aurora-lab keychain entry, the host key is pre-trusted, snippet hints are off
  start-app.sh    (re)start the app on :81, window 1280x800, type the vault password
  prep-lab.sh     demo files + coloured prompt (hostname derived from the target IP) on the aurora account
  logwriter.sh    start|stop - appends a log line every 0.6 s to ~aurora/demo/logs/app.log
  themes.list     theme ids and the names used in the palette

Notes
  * The UI language is English (settings.language = en in the demo data).
  * Timing: the app renders in software (LIBGL_ALWAYS_SOFTWARE=1) so xdotool pauses are generous
    (mouse hover frame before a click, 90-120 ms per typed character); a faster machine can shorten them.
  * Typing non-ASCII needs LC_ALL=C.UTF-8 (set in lib.sh) - "Rosé Pine" otherwise fails.
