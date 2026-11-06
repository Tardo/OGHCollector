// Copyright Alexandre D. Díaz
// UI texts built in JS. The page language is the server's choice (<html lang>, see
// crates/server/src/i18n.rs); a missing key shows the English text. `{0}`, `{1}`... take `args`.
const CATALOG = {
  es: {
    'Loading...': 'Cargando...',
    'Loading…': 'Cargando…',
    'Failed to load logs.': 'No se pudieron cargar los registros.',
    'Server responded with {0}': 'El servidor respondió con {0}',
    'Search module ({0})...': 'Buscar módulo ({0})...',
    'No modules found in the file.': 'No se encontraron módulos en el archivo.',
    '❓ Unknown to this system, not reported as missing: {0}':
      '❓ Desconocidos para este sistema, no se informan como faltantes: {0}',
    'Resume layout': 'Reanudar disposición',
    'Pause layout': 'Pausar disposición',
    'Localization: {0} - Odoo Module Metadata':
      'Localización: {0} - Odoo Module Metadata',
    'Remove {0}': 'Quitar {0}',
    'Odoo version this pack was built for':
      'Versión de Odoo para la que se creó este pack',
    Export: 'Exportar',
    "Export for Odoo {0} — open the pack's share link to convert it to another version first":
      'Exportar para Odoo {0} — abre el enlace para compartir del pack para convertirlo antes a otra versión',
    Share: 'Compartir',
    'Open a shareable link (and QR code) for this pack':
      'Abrir un enlace para compartir (y código QR) de este pack',
    'More actions': 'Más acciones',
    Rename: 'Renombrar',
    'Delete pack': 'Eliminar pack',
    'Delete pack "{0}"?': '¿Eliminar el pack "{0}"?',
    'Still loading Odoo versions - try again in a second.':
      'Aún se cargan las versiones de Odoo: inténtalo de nuevo en un segundo.',
    'License review': 'Revisión de licencias',
    'Checking…': 'Comprobando…',
    ' Copied!': ' ¡Copiado!',
    ' Press Ctrl+C': ' Pulsa Ctrl+C',
    ' Copy': ' Copiar',
    'Copied!': '¡Copiado!',
    'Press Ctrl+C': 'Pulsa Ctrl+C',
    Copy: 'Copiar',
    'Not available for Odoo {0}': 'No disponible para Odoo {0}',
    '+ New pack…': '+ Nuevo pack…',
    '{0} module(s) not found in {1}': '{0} módulo(s) no encontrado(s) en {1}',
    'Could not load the modules.': 'No se pudieron cargar los módulos.',
    '{0} module(s)': '{0} módulo(s)',
    'Failed to resolve dependencies for this pack.':
      'No se pudieron resolver las dependencias de este pack.',
    'No modules found for Odoo {0} in this pack.':
      'No se encontraron módulos para Odoo {0} en este pack.',
    'Processing…': 'Procesando…',
    '{0} critical': '{0} críticos',
    '{0} high severity': '{0} de severidad alta',
    'Highest observed severity': 'Severidad más alta observada',
    Unknown: 'Desconocido',
  },
  gl: {
    'Loading...': 'Cargando...',
    'Loading…': 'Cargando…',
    'Failed to load logs.': 'Non se puideron cargar os rexistros.',
    'Server responded with {0}': 'O servidor respondeu con {0}',
    'Search module ({0})...': 'Buscar módulo ({0})...',
    'No modules found in the file.': 'Non se atoparon módulos no ficheiro.',
    '❓ Unknown to this system, not reported as missing: {0}':
      '❓ Descoñecidos para este sistema, non se informan como faltantes: {0}',
    'Resume layout': 'Retomar disposición',
    'Pause layout': 'Pausar disposición',
    'Localization: {0} - Odoo Module Metadata':
      'Localización: {0} - Odoo Module Metadata',
    'Remove {0}': 'Quitar {0}',
    'Odoo version this pack was built for':
      'Versión de Odoo para a que se creou este pack',
    Export: 'Exportar',
    "Export for Odoo {0} — open the pack's share link to convert it to another version first":
      'Exportar para Odoo {0} — abre a ligazón para compartir do pack para convertelo antes a outra versión',
    Share: 'Compartir',
    'Open a shareable link (and QR code) for this pack':
      'Abrir unha ligazón para compartir (e código QR) deste pack',
    'More actions': 'Máis accións',
    Rename: 'Renomear',
    'Delete pack': 'Eliminar pack',
    'Delete pack "{0}"?': 'Eliminar o pack "{0}"?',
    'Still loading Odoo versions - try again in a second.':
      'Aínda se cargan as versións de Odoo: téntao de novo nun segundo.',
    'License review': 'Revisión de licenzas',
    'Checking…': 'Comprobando…',
    ' Copied!': ' Copiado!',
    ' Press Ctrl+C': ' Preme Ctrl+C',
    ' Copy': ' Copiar',
    'Copied!': 'Copiado!',
    'Press Ctrl+C': 'Preme Ctrl+C',
    Copy: 'Copiar',
    'Not available for Odoo {0}': 'Non dispoñible para Odoo {0}',
    '+ New pack…': '+ Novo pack…',
    '{0} module(s) not found in {1}': '{0} módulo(s) non atopado(s) en {1}',
    'Could not load the modules.': 'Non se puideron cargar os módulos.',
    '{0} module(s)': '{0} módulo(s)',
    'Failed to resolve dependencies for this pack.':
      'Non se puideron resolver as dependencias deste pack.',
    'No modules found for Odoo {0} in this pack.':
      'Non se atoparon módulos para Odoo {0} neste pack.',
    'Processing…': 'Procesando…',
    '{0} critical': '{0} críticos',
    '{0} high severity': '{0} de severidade alta',
    'Highest observed severity': 'Severidade máis alta observada',
    Unknown: 'Descoñecido',
  },
};

export function t(text, ...args) {
  const lang = document.documentElement.lang;
  const out = (CATALOG[lang] && CATALOG[lang][text]) || text;
  return out.replace(/\{(\d+)\}/g, (m, i) => (i < args.length ? args[i] : m));
}
