// Núcleo de resonda: lo usan la CLI (main.rs) y la interfaz gráfica (a través de ffi.rs).

pub mod ffi;
mod io;
pub mod media;
pub mod pipeline;
pub mod sources;
#[cfg(test)]
pub(crate) mod testutil;

use std::time::Duration;

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Error especial: la operación se canceló a petición del usuario (no es un fallo).
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelado")
    }
}

impl std::error::Error for Cancelled {}

pub fn new_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into()
}

/// Texto claro para un error. Los de red (`ureq`) y los de disco vienen en inglés técnico; aquí se
/// traducen a algo que el usuario pueda entender y arreglar.
pub fn explain(e: &(dyn std::error::Error + 'static)) -> String {
    if let Some(u) = e.downcast_ref::<ureq::Error>() {
        return explain_net(u);
    }
    if let Some(io) = e.downcast_ref::<std::io::Error>() {
        return explain_io(io);
    }
    e.to_string()
}

fn explain_io(io: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match io.kind() {
        StorageFull => "No hay espacio libre en el disco".into(),
        PermissionDenied => format!("Sin permiso para escribir en esa carpeta ({io})"),
        NotFound => format!("No se encontró la carpeta o el archivo ({io})"),
        ReadOnlyFilesystem => "La carpeta de destino es de solo lectura".into(),
        ConnectionRefused => "El servidor rechazó la conexión".into(),
        ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof => "Se cortó la conexión con el servidor".into(),
        TimedOut | WouldBlock => "Se agotó el tiempo de espera de la red".into(),
        _ => io.to_string(),
    }
}

fn explain_net(u: &ureq::Error) -> String {
    use ureq::Error::*;
    match u {
        Timeout(_) => "Se agotó el tiempo de espera de la red".into(),
        HostNotFound => "No se encontró el servidor: revisa tu conexión a internet".into(),
        ConnectionFailed => "No se pudo conectar con el servidor: revisa tu conexión a internet".into(),
        TooManyRedirects | RedirectFailed => "El servidor redirige en bucle o a un sitio inválido".into(),
        Tls(_) | Pem(_) => format!("Error de seguridad en la conexión ({u})"),
        InvalidProxyUrl => "La variable de proxy (HTTPS_PROXY / ALL_PROXY) no es una dirección válida".into(),
        BadUri(_) => "El enlace no es una dirección válida".into(),
        Io(io) => explain_io(io),
        other => format!("Error de red: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errores_de_disco_y_red_en_castellano() {
        let e = |k: std::io::ErrorKind| explain(&std::io::Error::from(k));
        assert!(e(std::io::ErrorKind::StorageFull).contains("espacio"));
        assert!(e(std::io::ErrorKind::PermissionDenied).contains("permiso"));
        assert!(e(std::io::ErrorKind::BrokenPipe).contains("conexión"));
        assert!(explain(&ureq::Error::HostNotFound).contains("internet"));
        assert!(explain(&ureq::Error::ConnectionFailed).contains("conectar"));
        assert!(explain(&ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut))).contains("tiempo"));
        // lo que no se reconoce se deja tal cual
        assert_eq!(explain(&*Box::<dyn std::error::Error + Send + Sync>::from("hola")), "hola");
    }
}
