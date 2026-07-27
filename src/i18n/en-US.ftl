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
session-creating-title = Starting stream
session-stop-button = Stop session
session-preparing-game = Preparing a session for "{ $game }"...
session-preparing = Preparing session...
session-queue-position = Position in NVIDIA's queue: #{ $position }
session-eta-minutes = Estimated wait: ~{ $minutes } min { $seconds } s
session-eta-seconds = Estimated wait: ~{ $seconds } seconds
session-queue-live = Refreshing status live (check { $attempt })...
session-connecting-attempt = Connecting to NVIDIA's server (check { $attempt })...
session-waiting-ready = Waiting for NVIDIA's server to be ready...
session-exit-hint = Tap "Stop session" or press Back (O) to confirm exit

# Session ready (debug/transition screen)
session-ready-title = Session ready
session-game = Game: { $game }
session-id = Session ID: { $id }
session-server-ip = Server IP: { $ip }
session-signaling = Signaling: { $server }
session-signaling-url = Signaling URL: { $url }
session-resolution = Resolution: { $value }
session-fps = FPS: { $value }
session-codec = Codec: { $value }
session-ready-hint = Press Confirm (X) to connect NVIDIA's signaling.
session-ready-footer = Confirm (X) to connect · Tap "Stop session" to exit

# WebRTC signaling
signaling-title = Signaling
signaling-session = Session: { $id }
signaling-offer-received = Offer SDP received ({ $bytes } bytes).
signaling-waiting-offer = Waiting for the offer SDP from the GFN server...

# Exit confirmation
exit-window-title = Confirm exit
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
