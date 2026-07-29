# Login / device-code flow
login-subtitle = Unofficial GeForce NOW client for PS Vita
login-hint = Press Confirm (X) to sign in with your NVIDIA account.
login-last-input = Last input detected: { $input }
login-requesting-code = Requesting an access code from NVIDIA...

device-title = Sign in on another device
device-step-open = 1. Open this address in the browser on your phone or computer:
device-step-scan = 2. Or scan the QR code and enter this code:
device-waiting = Waiting for you to finish signing in... (Back to cancel)

# Catalog
catalog-welcome = Welcome, { $name }
catalog-loading = Loading your GeForce NOW catalog...
catalog-search-hint = Search games...
catalog-search-button = Search
catalog-library-title = CLOUD TITLES
catalog-sort-button = Sort: { $sort }
catalog-sort-last-played = Recently Played
catalog-sort-relevance = Recommended
catalog-sort-title-asc = Title (A-Z)
catalog-sort-title-desc = Title (Z-A)
catalog-no-games-api = No games available were found (the API returned none).
catalog-no-match = No games match your search.
catalog-footer-hint = Up/Down to browse · Confirm (X) to play · Back (O) to clear the search
catalog-count = { $shown } of { $total }
catalog-count-loading = { $shown } of { $total } · loading more...

# Detail panel (right-hand side of the catalog)
detail-play = PLAY
detail-app-id = App ID: { $id }
detail-last-played = Last played: { $date }
detail-never-played = Never played on this account
detail-press = Press
detail-to-start = to start
detail-play-hint = Press Confirm (X) or tap PLAY to start streaming this game.
detail-empty = Select a game from the list to see its details.

# Session creation / queue
session-stop-button = Stop session
session-queue-position = Position in NVIDIA's queue: #{ $position }
session-eta-minutes = Estimated wait: ~{ $minutes } min { $seconds } s
session-eta-seconds = Estimated wait: ~{ $seconds } seconds
session-queue-live = Refreshing status live (check { $attempt })...
session-connecting-attempt = Connecting to NVIDIA's server (check { $attempt })...
session-waiting-ready = Waiting for NVIDIA's server to be ready...
session-server-busy = NVIDIA's servers are busy
session-server-busy-retry = Retrying... (attempt { $attempt })
session-cancel-button = Cancel session
session-exit-hint = Tap "Cancel session" or press (O) to confirm exit
session-now-loading = Now loading
session-step-queue = Queue
session-step-setup = Setup
session-step-ready = Ready
session-preparing-rig = Preparing your cloud rig
session-ready-headline = Your rig is ready

# Session ready (debug/transition screen)
session-ready-hint = Press Confirm (X) to connect NVIDIA's signaling.

# WebRTC signaling
signaling-title = Signaling
signaling-offer-received = Offer SDP received ({ $bytes } bytes).
signaling-waiting-offer = Waiting for the offer SDP from the GFN server...

# Exit confirmation
exit-heading = Stop the streaming session?
exit-body = Are you sure you want to leave and cancel the active GeForce NOW session?
exit-cancel = Back to the session
exit-confirm = Yes, exit and stop

# Streaming
streaming-game = Streaming "{ $game }"
streaming-generic = Streaming game...
streaming-signaling-done = WebRTC signaling and SDP exchange complete
streaming-waiting-negotiation = Waiting for WebRTC negotiation...

# Errors
error-title = Error
error-hint = Confirm or Back to return.
error-game-not-found = The selected game could not be found.

# Errors and status notes (built in app/mod.rs)
error-login-start = Could not start the login: { $error }
error-login-code-expired = The code expired before the login completed. Try again.
error-login-denied = Login was denied.
error-login-check = Login check failed: { $error }
error-profile-read = Login succeeded but the profile could not be read: { $error }
error-session-expired = Your session expired. Please sign in again.
error-catalog-load = Could not load your game library: { $error }
error-session-create = Could not start the stream: { $error }
error-signaling-disconnected = Signaling disconnected: { $reason }
error-stream-lost = Streaming connection lost: { $reason }
status-search-results = { $count } result(s) for "{ $query }"
status-search-failed = Search failed: { $error }
status-stream-live = Live video stream active
status-peer-error = Peer: { $error }
status-signaling-connected = Signaling connected, waiting for the SDP offer...
status-offer-received = SDP offer received ({ $bytes } bytes). Negotiating WebRTC...
status-remote-ice = Remote ICE candidate received from NVIDIA: { $candidate }
status-session-start-failed = Could not start the session: login or game missing.
status-signaling-connecting = Connecting to NVIDIA signaling...
status-signaling-connect-failed = Could not connect signaling: { $error }

settings-fps-heading = Stream frame rate
settings-fps-60 = 60 fps - smoother motion
settings-fps-30 = 30 fps - sharper picture
settings-trigger-heading = Rear-panel L2/R2 pressure
settings-audio-boost-heading = Volume boost
session-keyboard-show = Keyboard
session-keyboard-hide = Hide keyboard
key-esc = Esc
key-tab = Tab
key-enter = Enter
key-shift = Shift
key-ctrl = Ctrl
key-alt = Alt
key-f1 = F1
key-f2 = F2
key-f3 = F3
key-f4 = F4
settings-heading = Settings
settings-title = Settings
account-close = Close
settings-language-heading = Language
controls-hint-heading = Vita controls
controls-hint-rear = The rear panel stands in for the buttons this console does not have:
controls-hint-touch = The front touchscreen moves the mouse; tap to click.
controls-hint-dismiss = Got it
settings-stick-zones-heading = L3/R3 on the front screen
settings-stick-zones-off = Off
settings-stick-zones-hidden = On
settings-stick-zones-visible = On + show
controls-hint-sticks = The bottom corners of the screen are L3 and R3.
error-session-busy-title = A session is already open
error-session-busy-body = GeForce NOW still has a session running for this account, and it is not one this app can close. Fastest fix: open play.geforcenow.com and start a game there to take it over. Otherwise sign out of GeForce NOW on your other devices, or wait about 8 minutes for it to time out.
error-auth-title = Sign in again
error-auth-body = Your NVIDIA authorization expired. Sign in to this account again to carry on.
