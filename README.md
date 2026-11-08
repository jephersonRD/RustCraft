<p align="center">
  <img src="RustCraft.png" width="112" alt="RustCraft">
</p>

<h1 align="center">RustCraft</h1>

<p align="center">
  <strong>Lanzador offline para Horizon, la reescritura de Minecraft Java Edition en Rust.</strong><br>
  Sin cuentas de Microsoft, sin telemetría y sin pasos innecesarios.
</p>

<p align="center">
  <a href="https://github.com/jephersonRD/RustCraft/releases"><img src="https://img.shields.io/badge/versi%C3%B3n-0.1.0-f7c614?style=flat-square" alt="Versión"></a>
  <img src="https://img.shields.io/badge/Minecraft-26.3-3dc75b?style=flat-square" alt="Minecraft 26.3">
  <img src="https://img.shields.io/badge/Java-25-e76f00?style=flat-square" alt="Java 25">
  <img src="https://img.shields.io/badge/licencia-MIT-4c8bf5?style=flat-square" alt="MIT">
</p>

---

## ¿Qué es RustCraft?

**RustCraft** es un lanzador de escritorio para [Horizon](https://github.com/horizon-rust/horizon), el motor de Minecraft Java Edition escrito en Rust. Se encarga de todo lo que hay entre tú y el juego:

- Descarga y verifica la versión de Horizon que elijas.
- Descarga los archivos oficiales de Minecraft desde Mojang y comprueba cada SHA-1.
- Prepara los recursos del juego con `horizon-prepare` y Java 25.
- Crea instancias independientes con sus propios mundos y ajustes.
- Te deja jugar con un **nombre offline** y un UUID estable, sin login.

La interfaz está pensada para no pedirte nada más que un nombre la primera vez. El resto se hace desde **Biblioteca**.

## Compatibilidad

| Componente | Versión |
| --- | --- |
| RustCraft | 0.1.0 |
| Horizon | `nightly` y releases compatibles |
| Minecraft Java Edition | **26.3** |
| Java runtime | **25** |
| Sistemas | Linux x86_64 · Windows x64/ARM64 · macOS Intel/Apple Silicon |

> La versión de Minecraft la fija Horizon. RustCraft consulta su API de preparación y descarga exactamente los archivos que esa versión necesita.

## Instalación rápida

Los scripts detectan el sistema operativo, la arquitectura y —en Linux— la distribución y el gestor de paquetes. Al ejecutarlos muestran tres opciones:

1. **Instalar**
2. **Actualizar**
3. **Desinstalar**

### Linux y macOS

```bash
curl -fsSL https://raw.githubusercontent.com/jephersonRD/RustCraft/main/install.sh -o install.sh
chmod +x install.sh
./install.sh
```

También puedes ejecutar una acción directamente:

```bash
./install.sh install
./install.sh update
./install.sh uninstall
```

El binario se instala en `~/.local/bin/rustcraft` y los datos en `~/.local/share/RustCraft`. En Linux, el instalador también crea la entrada **RustCraft** en el menú de aplicaciones con su icono; al desinstalar, ambas se eliminan.

### Windows

Abre PowerShell y ejecuta:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/jephersonRD/RustCraft/main/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

O directamente una acción:

```powershell
.\install.ps1 install
.\install.ps1 update
.\install.ps1 uninstall
```

El binario se instala en `%LOCALAPPDATA%\RustCraft\bin` y los datos en `%APPDATA%\RustCraft`.

> Si no existe un binario precompilado para tu plataforma, el script compila desde el código fuente usando Cargo. Necesitarás [Rust](https://rustup.rs) en ese caso.

## Primeros pasos

1. Abre **RustCraft** (sin argumentos abre la interfaz gráfica).
2. Escribe el nombre con el que quieres jugar. Es obligatorio y se guarda como cuenta offline.
3. En **Biblioteca**, pulsa **Actualizar** y elige una versión de Horizon.
4. Espera a que termine la barra de instalación; verás porcentaje, megabytes descargados, total y lo que falta.
5. Pulsa **Preparar** para descargar Minecraft y generar los recursos.
6. Pulsa **Jugar**.

El nombre debe tener entre 3 y 16 caracteres: letras, números o `_`.

## Uso desde la terminal

```text
rustcraft                 # Abre la interfaz gráfica
rustcraft doctor          # Comprueba rutas, nombre y Java
rustcraft paths           # Muestra todas las carpetas que usa
rustcraft name <NOMBRE>   # Guarda el nombre offline
rustcraft versions        # Versiones de Horizon instaladas
rustcraft versions --remote
rustcraft install [TAG]   # Instala Horizon (por defecto, nightly)
rustcraft minecraft [VER] # Descarga Minecraft y verifica SHA-1
rustcraft java [RUTA]     # Detecta o guarda el Java 25
rustcraft prepare         # Prepara los recursos de Horizon
rustcraft instance list
rustcraft instance create <NOMBRE>
rustcraft instance remove <NOMBRE>
rustcraft play [INSTANCIA]
rustcraft gui
```

Ejemplo completo:

```bash
rustcraft name Alex
rustcraft install nightly
rustcraft prepare
rustcraft play
```

Usa `--data-dir <DIR>` o la variable `RUSTCRAFT_DATA_DIR` para separar perfiles de datos.

## Dónde se guarda todo

| Sistema | Carpeta de datos |
| --- | --- |
| Linux | `~/.local/share/RustCraft` |
| Windows | `%APPDATA%\RustCraft` |
| macOS | `~/Library/Application Support/RustCraft` |

Dentro encontrarás `config.json`, las versiones de Horizon, los archivos de Minecraft, los recursos preparados y las instancias.

## Compilar desde el código fuente

Requisitos: Rust 1.85 o superior y las dependencias de sistema de `eframe`/`egui`.

```bash
git clone https://github.com/jephersonRD/RustCraft.git
cd RustCraft
cargo build --release
./target/release/rustcraft
```

Comprobaciones:

```bash
cargo check
cargo test
```

## Estructura del proyecto

```text
src/
├── main.rs               Punto de entrada
├── cli.rs                Comandos de terminal
├── gui.rs                Interfaz gráfica (egui/eframe)
├── accounts_offline.rs   Nombres y UUID offline
├── instances.rs          Instancias y mundos
├── java.rs               Detección de Java 25
├── launch.rs             Arranque de horizon-client
├── prepare.rs            Preparación de recursos
├── mojang.rs             Descarga oficial de Minecraft
├── github_releases.rs    Releases de Horizon
└── core/                 Rutas, descargas, hashes, configuración y errores
```

## Problemas frecuentes

**No aparece Java 25.** RustCraft necesita Java 25 para `horizon-prepare`. El instalador detecta tu distribución y te muestra el comando adecuado; también puedes instalar [Temurin 25](https://adoptium.net).

**La preparación falla o faltan recursos.** Vuelve a ejecutar `rustcraft prepare --force`. Si el problema continúa, revisa que la descarga de Mojang haya terminado correctamente.

**El juego no conecta.** Horizon rechaza algunas conexiones a través de VPN o proxy. RustCraft limpia esas variables al lanzar el juego, pero una VPN a nivel de sistema puede seguir interfiriendo.

**No aparecen las noticias.** Se leen de [`Noticia.md`](Noticia.md) en la rama `main`. Comprueba tu conexión y pulsa **Actualizar** en Biblioteca.

## Contribuir

Las issues y pull requests son bienvenidas. Antes de enviar cambios, ejecuta `cargo test` y describe el problema que resuelve el cambio.

## Licencia

Distribuido bajo la licencia **MIT**. Consulta `Cargo.toml` para más detalles.

<p align="center"><sub>Hecho con Rust para la comunidad de Horizon.</sub></p>
