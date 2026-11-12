//! Interface language. Russian is the source: every string is written in Russian and looked up here
//! when English is on. Whole strings match first; messages built at run time (engine status, OBS errors)
//! are translated by their fixed fragments.

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    EN.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    EN.load(Ordering::Relaxed)
}

/// The string in the current language.
pub fn tr(s: &str) -> String {
    if !is_english() {
        return s.to_string();
    }
    to_english(s)
}

/// English for a Russian string: an exact match, else its known fragments replaced.
pub fn to_english(s: &str) -> String {
    if let Some(en) = exact(s) {
        return en.to_string();
    }
    let mut out = s.to_string();
    for (ru, en) in FRAGMENTS {
        if out.contains(ru) {
            out = out.replace(ru, en);
        }
    }
    out
}

fn exact(s: &str) -> Option<&'static str> {
    EXACT.iter().find(|(ru, _)| *ru == s).map(|(_, en)| *en)
}

const EXACT: &[(&str, &str)] = &[
    // window, header
    ("В кадре", "In Frame"),
    ("Включить слежение", "Start tracking"),
    ("Выключить слежение", "Stop tracking"),
    ("Общий", "Wide"),
    ("Цель", "Target"),
    ("Зона", "Zone"),
    ("Сброс зоны", "Reset zone"),
    ("Плашка", "Name plate"),
    ("Настройки", "Settings"),
    ("Остановлено", "Stopped"),
    ("Ждёт сцену", "Waiting for scene"),
    ("Ведёт", "Following"),
    ("OBS не отвечает", "OBS not responding"),
    ("OBS: нужен пароль", "OBS: password needed"),
    ("OBS: ошибка связи", "OBS: connection error"),
    // monitors
    ("Камера", "Camera"),
    ("Эфир", "Program"),
    ("Нет сигнала", "No signal"),
    (" — проверьте камеру", " — check the camera"),
    ("Обведите зону сцены мышью", "Drag to mark the stage zone"),
    ("Слежение не запущено", "Tracking is off"),
    ("Нажмите «Включить слежение» или F5", "Press «Start tracking» or F5"),
    ("Запуск", "Starting"),
    ("Загружаю нейросеть и подключаюсь к OBS…", "Loading the model and connecting to OBS…"),
    ("Ни одна из отмеченных сцен не в эфире — слежение ждёт", "None of the chosen scenes is on air — waiting"),
    ("Ищу спикера", "Looking for the speaker"),
    ("Общий план", "Wide shot"),
    ("Общий план сцены", "Stage wide shot"),
    ("Дуэт", "Duo"),
    ("Дуэт · ищу второго", "Duo · looking for the second"),
    ("Слежение", "Tracking"),
    ("Спикер", "Speaker"),
    ("Второй", "Second"),
    // readouts
    ("Зум", "Zoom"),
    ("План", "Shot"),
    ("Узнаю", "Match"),
    ("Крупный", "Close"),
    ("Средний", "Medium"),
    ("Норма", "OK"),
    ("Сбой", "Fault"),
    // scenes
    ("Сцены", "Scenes"),
    ("Не трогать", "Don't touch"),
    ("Вести", "Follow"),
    ("Нет этой камеры", "No such camera"),
    ("Подключитесь к OBS, чтобы увидеть сцены", "Connect to OBS to see the scenes"),
    ("В OBS нет сцен", "OBS has no scenes"),
    // tabs
    ("Кадр", "Frame"),
    ("Движение", "Motion"),
    // frame tab
    ("Источник", "Source"),
    ("Грудь", "Chest"),
    ("Пояс", "Waist"),
    ("Бёдра", "Hips"),
    ("Колено", "Knee"),
    ("Фикс.", "Fixed"),
    ("Рост", "Full"),
    ("Макс. зум", "Max zoom"),
    ("Над головой", "Headroom"),
    ("Монтаж", "Cutting"),
    ("Два плана из одной камеры", "Two shots from one camera"),
    ("Не чаще", "At most every"),
    // motion tab
    ("Плавность", "Smoothness"),
    ("Мёртвая зона", "Dead zone"),
    ("Скорость", "Speed"),
    ("Плавность зума", "Zoom smoothness"),
    // speaker tab
    ("Узнавать по одежде", "Recognize by clothes"),
    ("Пропал — общий план", "Lost — go wide"),
    ("Через", "After"),
    // plate tab
    ("Имя", "Name"),
    ("Подпись", "Title"),
    ("Алексей Смирнов", "John Smith"),
    ("Пастор церкви", "Church pastor"),
    ("Вид", "Look"),
    ("Тёмный", "Dark"),
    ("Светлый", "Light"),
    ("Цветной", "Color"),
    ("Шторка", "Wipe"),
    ("Сбоку", "Slide"),
    ("Снизу", "Rise"),
    ("Плавно", "Fade"),
    ("На экране", "On screen"),
    ("Показать в эфире", "Show on air"),
    ("Превью", "Preview"),
    // camera tab
    ("Сторож камеры", "Camera watchdog"),
    ("Следить за камерой", "Watch the camera"),
    ("Звук тревоги", "Alarm sound"),
    ("Уводить эфир на запасную сцену", "Switch to a backup scene"),
    ("Производительность", "Performance"),
    ("Видеокарта", "GPU"),
    ("Кадров нейросети в секунду", "Detections per second"),
    ("Устройство", "Device"),
    // settings dialog
    ("Язык", "Language"),
    ("Подключение к OBS", "OBS connection"),
    (
        "В OBS: Инструменты → Настройки сервера WebSocket. Адрес, порт и пароль — оттуда же.",
        "In OBS: Tools → WebSocket Server Settings. Address, port and password are there.",
    ),
    ("Адрес", "Address"),
    ("Порт", "Port"),
    ("Пароль", "Password"),
    ("без пароля", "no password"),
    ("Проверить", "Test"),
    ("Проверяю…", "Testing…"),
    ("Подключаться при запуске", "Connect on start"),
    ("Сразу включать слежение", "Start tracking on launch"),
    ("Запускать свёрнутым", "Start minimized"),
    ("Горячие клавиши", "Hot keys"),
    ("Пробел", "Space"),
    ("Клик", "Click"),
    ("включить / выключить слежение", "start / stop tracking"),
    ("общий план", "wide shot"),
    ("показать плашку", "show the name plate"),
    ("выбрать спикера на камере", "pick the speaker on the camera"),
    ("Отмена", "Cancel"),
    ("Сохранить", "Save"),
    ("Понятно", "OK"),
    // console layout
    ("Слежение остановлено", "Tracking stopped"),
    ("Включить", "Start"),
    ("Выключить", "Stop"),
    ("ВИД", "VIEW"),
    ("ЗОНА", "ZONE"),
    ("Выделить", "Mark"),
    ("Сбросить", "Reset"),
    ("СЦЕНЫ OBS", "OBS SCENES"),
    ("Обновить", "Refresh"),
    ("Поведение", "Behaviour"),
    (" · В ЭФИРЕ", " · ON AIR"),
    (" · ВЫБРАНА", " · SELECTED"),
    (" · НЕТ КАМЕРЫ", " · NO CAMERA"),
    ("НЕТ СИГНАЛА", "NO SIGNAL"),
    ("Нет связи с OBS", "No connection to OBS"),
    (
        "Запустите OBS и включите WebSocket: Инструменты → Настройки сервера WebSocket. Затем нажмите «Включить слежение» или F5.",
        "Start OBS and turn on WebSocket: Tools → WebSocket Server Settings. Then press «Start tracking» or F5.",
    ),
    ("Подключиться снова", "Reconnect"),
    ("Как настроить", "How to set up"),
    ("Общий и крупный — из одного источника", "Wide and close — from one source"),
    ("Чёрный кадр, зависшая картинка, нет сигнала", "Black frame, frozen picture, no signal"),
    // name plate preview
    ("Предпросмотр", "Preview"),
    ("ПРЕДПРОСМОТР", "PREVIEW"),
    ("ИДЁТ ПОКАЗ", "PLAYING"),
    ("ИМЯ ФАМИЛИЯ", "JOHN SMITH"),
    ("Проиграть", "Play"),
    ("Остановить", "Stop"),
    // status
    ("Готово", "Ready"),
    ("Цель выбрана", "Target selected"),
    ("Второй человек выбран", "Second person selected"),
    ("Камера снова в порядке", "Camera is fine again"),
    ("Загрузка нейросети…", "Loading the model…"),
    ("Подключение к OBS…", "Connecting to OBS…"),
    ("Нет связи с OBS — запущен ли OBS и включён ли WebSocket?", "No connection to OBS — is OBS running with WebSocket on?"),
    ("Выберите камеру и отметьте, в каких сценах работать.", "Choose a camera and the scenes to work in."),
    ("Потеряно соединение с OBS.", "Lost connection to OBS."),
    ("Впишите имя для плашки (вкладка «Плашка»)", "Enter a name for the plate (Name plate tab)"),
    ("Камера показывает чёрный кадр", "The camera shows a black frame"),
    ("Картинка с камеры зависла", "The camera picture froze"),
    ("Нет сигнала с камеры", "No signal from the camera"),
];

/// Pieces of messages built at run time; longer pieces first.
const FRAGMENTS: &[(&str, &str)] = &[
    ("OBS просит пароль — впишите его в «Подключение»", "OBS asks for a password — enter it in Settings"),
    (". Кнопка подключения — справа вверху.", ". Settings are at the top right."),
    ("неверный пароль OBS WebSocket", "wrong OBS WebSocket password"),
    ("Внимание: источник ", "Note: source "),
    (" — приближение будет мягким", " — zoom will look soft"),
    ("Камера пропала — включена сцена ", "Camera lost — switched to scene "),
    ("Не удалось восстановить источник в ", "Could not restore the source in "),
    ("Не удалось показать плашку: ", "Could not show the plate: "),
    ("Не удалось открыть превью: ", "Could not open the preview: "),
    ("Не удалось подключиться: ", "Could not connect: "),
    (" не отдаёт картинку. Камера включена?", " gives no picture. Is the camera on?"),
    (" нет источника ", " has no source "),
    ("В сцене ", "Scene "),
    ("Источник ", "Source "),
    ("OBS не отвечает на ", "OBS is not responding at "),
    ("OBS закрыл соединение", "OBS closed the connection"),
    ("OBS не ответил вовремя", "OBS did not answer in time"),
    ("связь с OBS потеряна: ", "lost connection to OBS: "),
    ("неизвестный адрес ", "unknown address "),
    ("ошибка WebSocket: ", "WebSocket error: "),
    ("Модель не найдена: ", "Model not found: "),
    ("Подключено к ", "Connected to "),
    ("Подключено: ", "Connected: "),
    (" · камера: ", " · camera: "),
    ("Работает · ", "Running · "),
    ("Ведёт · ", "Following · "),
    ("Плашка: ", "Name plate: "),
    ("Ошибка: ", "Error: "),
    (" · крупный план", " · close shot"),
    (" · средний план", " · medium shot"),
    ("Слежение", "Tracking"),
    ("Спикер ", "Speaker "),
    (" мс", " ms"),
    (" к/с", " fps"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_fragments() {
        assert_eq!(to_english("Кадр"), "Frame");
        assert_eq!(to_english("Работает · 3840×2160 → 1920×1080"), "Running · 3840×2160 → 1920×1080");
        assert_eq!(to_english("Слежение · крупный план"), "Tracking · close shot");
        assert_eq!(to_english("2.14×   18 мс   10 к/с"), "2.14×   18 ms   10 fps");
        assert_eq!(to_english("Подключено к OBS 33.0.1 · камера: Мультимедиа"), "Connected to OBS 33.0.1 · camera: Мультимедиа");
        // names from OBS stay as they are
        assert_eq!(to_english("Мультимедиа"), "Мультимедиа");
    }

    #[test]
    fn every_slint_string_has_english() {
        let slint = include_str!("../ui/app.slint");
        let mut missing = Vec::new();
        for part in slint.split("I18n.tr(\"").skip(1) {
            let key = &part[..part.find('"').unwrap()];
            if exact(key).is_none() {
                missing.push(key.to_string());
            }
        }
        assert!(missing.is_empty(), "no English for: {missing:?}");
    }
}
