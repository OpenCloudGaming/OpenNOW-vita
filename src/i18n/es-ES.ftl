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
session-stop-button = Detener sesión
session-queue-position = Posición en la cola de NVIDIA: n.º { $position }
session-eta-minutes = Espera estimada: ~{ $minutes } min { $seconds } s
session-eta-seconds = Espera estimada: ~{ $seconds } segundos
session-queue-live = Actualizando el estado en directo (comprobación { $attempt })...
session-connecting-attempt = Conectando con el servidor de NVIDIA (comprobación { $attempt })...
session-waiting-ready = Esperando a que el servidor de NVIDIA esté listo...
session-server-busy = Los servidores de NVIDIA están saturados
session-server-busy-retry = Reintentando... (intento { $attempt })
session-cancel-button = Cancelar sesión
session-exit-hint = Toca "Cancelar sesión" o pulsa (O) para confirmar la salida
session-now-loading = Cargando
session-step-queue = Cola
session-step-setup = Preparación
session-step-ready = Lista
session-preparing-rig = Preparando tu equipo en la nube
session-ready-headline = Tu equipo está listo

# Sesión lista (pantalla de depuración/transición)
session-ready-hint = Pulsa Confirmar (X) para conectar la señalización de NVIDIA.

# Señalización WebRTC
signaling-title = Señalización
signaling-offer-received = Offer SDP recibido ({ $bytes } bytes).
signaling-waiting-offer = Esperando el offer SDP del servidor de GFN...

# Confirmación de salida
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

# Errores y notas de estado (construidos en app/mod.rs)
error-login-start = No se pudo iniciar sesión: { $error }
error-login-code-expired = El código expiró antes de completar el login. Inténtalo de nuevo.
error-login-denied = Inicio de sesión rechazado.
error-login-check = Fallo comprobando el login: { $error }
error-profile-read = Login correcto pero no se pudo leer el perfil: { $error }
error-session-expired = Tu sesión ha expirado. Por favor, vuelve a iniciar sesión.
error-catalog-load = No se pudo cargar tu biblioteca de juegos: { $error }
error-session-create = No se pudo iniciar la transmisión: { $error }
error-signaling-disconnected = Señalización desconectada: { $reason }
error-stream-lost = Conexión de streaming perdida: { $reason }
status-search-results = { $count } resultado(s) para "{ $query }"
status-search-failed = Búsqueda falló: { $error }
status-stream-live = Transmisión de vídeo en directo activa
status-peer-error = Peer: { $error }
status-signaling-connected = Señalización conectada, esperando offer SDP...
status-offer-received = Offer SDP recibido ({ $bytes } bytes). Negociando WebRTC...
status-remote-ice = Candidato ICE remoto recibido de NVIDIA: { $candidate }
status-session-start-failed = No se pudo iniciar la sesión: falta login o juego.
status-signaling-connecting = Conectando a la señalización de NVIDIA...
status-signaling-connect-failed = No se pudo conectar la señalización: { $error }

settings-fps-heading = Fotogramas del stream
settings-fps-60 = 60 fps - movimiento más fluido
settings-fps-30 = 30 fps - imagen más nítida
settings-trigger-heading = Presión de L2/R2 en el panel trasero
settings-audio-boost-heading = Amplificacion de volumen
session-keyboard-show = Teclado
session-keyboard-hide = Ocultar teclado
key-esc = Esc
key-tab = Tab
key-enter = Intro
key-shift = Mayus
key-ctrl = Ctrl
key-alt = Alt
key-f1 = F1
key-f2 = F2
key-f3 = F3
key-f4 = F4
settings-heading = Ajustes
settings-title = Ajustes de controles
account-close = Cerrar
settings-language-heading = Idioma
controls-hint-heading = Controles de Vita
controls-hint-rear = El panel trasero sustituye a los botones que esta consola no tiene:
controls-hint-touch = La pantalla tactil mueve el raton; toca para hacer clic.
controls-hint-dismiss = Entendido
settings-stick-zones-heading = L3/R3 en la pantalla
settings-stick-zones-off = No
settings-stick-zones-hidden = Si
settings-stick-zones-visible = Si + ver
controls-hint-sticks = Las esquinas de abajo de la pantalla son L3 y R3.
error-session-busy-title = Ya hay una sesion abierta
error-session-busy-body = GeForce NOW sigue con una sesion activa en esta cuenta, y no es una que esta app pueda cerrar. Lo mas rapido: abre play.geforcenow.com y lanza un juego alli para tomar el control. Si no, cierra sesion de GeForce NOW en tus otros dispositivos, o espera unos 8 minutos a que caduque.
error-auth-title = Vuelve a iniciar sesion
error-auth-body = Tu autorizacion de NVIDIA ha caducado. Inicia sesion de nuevo con esta cuenta para continuar.
