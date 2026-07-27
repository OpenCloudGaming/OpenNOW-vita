# Login / flujo de código de dispositivo
login-subtitle = Cliente no oficial de GeForce NOW para PS Vita
login-hint = Pulsa Confirmar (X) para iniciar sesión con tu cuenta de NVIDIA.
login-last-input = Última entrada detectada: { $input }
login-requesting-code = Solicitando un código de acceso a NVIDIA...

device-title = Inicia sesión en otro dispositivo
device-step-open = 1. Abre esta dirección en el navegador de tu teléfono u ordenador:
device-step-scan = 2. O escanea el código QR e introduce este código:
device-waiting = Esperando a que completes el inicio de sesión... (Atrás para cancelar)

# Catálogo
catalog-welcome = Bienvenido, { $name }
catalog-loading = Cargando tu catálogo de GeForce NOW...
catalog-search-hint = Buscar juegos...
catalog-search-button = Buscar
catalog-library-title = JUEGOS EN LA NUBE
catalog-sort-button = Orden: { $sort }
catalog-sort-last-played = Jugado recientemente
catalog-sort-relevance = Recomendado
catalog-sort-title-asc = Título (A-Z)
catalog-sort-title-desc = Título (Z-A)
catalog-no-games-api = No se encontraron juegos disponibles (la API no devolvió ninguno).
catalog-no-match = Ningún juego coincide con la búsqueda.
catalog-footer-hint = Arriba/Abajo para navegar · Confirmar (X) para jugar · Atrás (O) para limpiar la búsqueda
catalog-count = { $shown } de { $total }
catalog-count-loading = { $shown } de { $total } · cargando más...

# Panel de detalle (lado derecho del catálogo)
detail-play = JUGAR
detail-app-id = ID de aplicación: { $id }
detail-last-played = Última partida: { $date }
detail-never-played = Nunca jugado con esta cuenta
detail-press = Pulsa
detail-to-start = para empezar
detail-play-hint = Pulsa Confirmar (X) o toca JUGAR para empezar a transmitir este juego.
detail-empty = Selecciona un juego de la lista para ver sus detalles.

# Creación de sesión / cola
session-creating-title = Iniciando transmisión
session-stop-button = Detener sesión
session-preparing-game = Preparando una sesión para "{ $game }"...
session-preparing = Preparando sesión...
session-queue-position = Posición en la cola de NVIDIA: n.º { $position }
session-eta-minutes = Espera estimada: ~{ $minutes } min { $seconds } s
session-eta-seconds = Espera estimada: ~{ $seconds } segundos
session-queue-live = Actualizando el estado en directo (comprobación { $attempt })...
session-connecting-attempt = Conectando con el servidor de NVIDIA (comprobación { $attempt })...
session-waiting-ready = Esperando a que el servidor de NVIDIA esté listo...
session-exit-hint = Toca "Detener sesión" o pulsa Atrás (O) para confirmar la salida

# Sesión lista (pantalla de depuración/transición)
session-ready-title = Sesión lista
session-game = Juego: { $game }
session-id = ID de sesión: { $id }
session-server-ip = IP del servidor: { $ip }
session-signaling = Señalización: { $server }
session-signaling-url = URL de señalización: { $url }
session-resolution = Resolución: { $value }
session-fps = FPS: { $value }
session-codec = Códec: { $value }
session-ready-hint = Pulsa Confirmar (X) para conectar la señalización de NVIDIA.
session-ready-footer = Confirmar (X) para conectar · Toca "Detener sesión" para salir

# Señalización WebRTC
signaling-title = Señalización
signaling-session = Sesión: { $id }
signaling-offer-received = Offer SDP recibido ({ $bytes } bytes).
signaling-waiting-offer = Esperando el offer SDP del servidor de GFN...

# Confirmación de salida
exit-window-title = Confirmar salida
exit-heading = ¿Detener la sesión de transmisión?
exit-body = ¿Seguro que quieres salir y cancelar la sesión activa de GeForce NOW?
exit-cancel = Volver a la sesión
exit-confirm = Sí, salir y detener

# Transmisión
streaming-game = Transmitiendo "{ $game }"
streaming-generic = Transmitiendo juego...
streaming-signaling-done = Señalización WebRTC e intercambio SDP completados
streaming-waiting-negotiation = Esperando la negociación WebRTC...

# Errores
error-title = Error
error-hint = Confirmar o Atrás para volver.
error-game-not-found = No se encontró el juego seleccionado.
