// What the installer says, in each language
//
// One struct with one field per phrase and one const per language, so a phrase
// missing from one language is a compile error. The screens hold no literal,
// only `m.lang.s().next`, so a new language is a single const here.
//
// The faces carry Cyrillic: `fontgen` builds U+0400-045F alongside ASCII.

extern crate alloc;
use alloc::string::String;
use alloc::format;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub fn s(self) -> &'static Strings {
        match self {
            Lang::En => &EN,
            Lang::Ru => &RU,
        }
    }

    /// Each language names itself in itself: somebody who cannot read the
    /// current language has to find their own
    pub fn own_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
        }
    }

    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];
}

pub struct Strings {
    // Welcome
    pub welcome_title: &'static str,
    pub welcome_1: &'static str,
    pub welcome_2: &'static str,
    pub language: &'static str,
    pub time: &'static str,
    pub next: &'static str,

    // Disk
    pub disk_title: &'static str,
    pub available: &'static str,
    pub available_sub: &'static str,
    pub no_disks: &'static str,
    pub free: &'static str,
    pub the_medium: &'static str,
    pub no_table: &'static str,
    pub refresh: &'static str,
    pub cancel: &'static str,
    pub back: &'static str,
    pub install: &'static str,

    // Progress
    pub installing: &'static str,
    pub preparing: &'static str,
    pub close: &'static str,

    // Done
    pub done_title: &'static str,
    pub done_1: &'static str,
    pub done_2: &'static str,
    pub done_3: &'static str,
    pub get_started: &'static str,

    // The steps the mechanism reports. Same order as
    // install_cmds::progress::Step
    pub step_idle: &'static str,
    pub step_part: &'static str,
    pub step_format: &'static str,
    pub step_copy: &'static str,
    pub step_boot: &'static str,
    pub step_done: &'static str,
    pub step_failed: &'static str,
}

pub const EN: Strings = Strings {
    welcome_title: "Welcome to MikuOS!",
    welcome_1: "Thank you for choosing MikuOS.",
    welcome_2: "Let's get your system set up.",
    language: "Language",
    time: "Time",
    next: "Next",

    disk_title: "Choose a disk",
    available: "Available disks",
    available_sub: "Pick the disk the system will be installed on",
    no_disks: "No disk on this machine answers.",
    free: "Free",
    the_medium: "the medium this is running from",
    no_table: "no partition table",
    refresh: "Refresh list",
    cancel: "Cancel",
    back: "Back",
    install: "Install",

    installing: "Installing MikuOS",
    preparing: "preparing the disk",
    close: "Close",

    done_title: "MikuOS is installed!",
    done_1: "I hope you enjoy using it, and that it doesn't",
    done_2: "break within the first three hours! :D",
    done_3: "Take the medium out and reboot.",
    get_started: "Get Started",

    step_idle: "waiting",
    step_part: "Partitioning the disk",
    step_format: "Creating the filesystem",
    step_copy: "Copying the system",
    step_boot: "Making it bootable",
    step_done: "Done",
    step_failed: "Failed",
};

pub const RU: Strings = Strings {
    welcome_title: "Добро пожаловать в MikuOS!",
    welcome_1: "Спасибо, что выбрали MikuOS.",
    welcome_2: "Давайте настроим систему.",
    language: "Язык",
    time: "Время",
    next: "Далее",

    disk_title: "Выбор диска",
    available: "Доступные диски",
    available_sub: "Выберите диск, на который будет установлена система",
    no_disks: "Ни один диск не отвечает.",
    free: "Свободно",
    the_medium: "носитель, с которого запущена система",
    no_table: "нет таблицы разделов",
    refresh: "Обновить список",
    cancel: "Отмена",
    back: "Назад",
    install: "Установить",

    installing: "Установка MikuOS",
    preparing: "подготовка диска",
    close: "Закрыть",

    done_title: "MikuOS установлена!",
    done_1: "Надеюсь, пользоваться ей будет приятно и она не",
    done_2: "сломается в первые три часа! :D",
    done_3: "Выньте носитель и перезагрузитесь.",
    get_started: "Начать",

    step_idle: "ожидание",
    step_part: "Разметка диска",
    step_format: "Создание файловой системы",
    step_copy: "Копирование системы",
    step_boot: "Установка загрузчика",
    step_done: "Готово",
    step_failed: "Ошибка",
};

// The phrases with something of the machine's in the middle of them
//
// Russian puts the number somewhere English does not, so each one is written
// out per language

/// The name of a step the install mechanism is on
pub fn step_name(l: Lang, step: crate::shell::commands::install_cmds::progress::Step) -> &'static str {
    use crate::shell::commands::install_cmds::progress::Step;
    let s = l.s();
    match step {
        Step::Idle => s.step_idle,
        Step::Partitioning => s.step_part,
        Step::Formatting => s.step_format,
        Step::Copying => s.step_copy,
        Step::Bootloader => s.step_boot,
        Step::Done => s.step_done,
        Step::Failed => s.step_failed,
    }
}

/// The line naming what is about to be destroyed: "the selected disk" could not
/// be checked
pub fn erase_warning(l: Lang, drive: usize, size: &str, install: &str) -> String {
    match l {
        Lang::En => format!(
            "Everything on drive {} ({}) will be erased. Press {} to go ahead.",
            drive, size, install),
        Lang::Ru => format!(
            "Всё на диске {} ({}) будет стёрто. Нажмите «{}», чтобы продолжить.",
            drive, size, install),
    }
}

pub fn selected_line(l: Lang, drive: usize, size: &str) -> String {
    match l {
        Lang::En => format!("Drive {} - {} selected", drive, size),
        Lang::Ru => format!("Выбран диск {} - {}", drive, size),
    }
}

/// A disk's second line: what it is attached by, and what is on it now
pub fn disk_subtitle(l: Lang, bus: &str, size: &str, partitions: u8, has_gpt: bool) -> String {
    if !has_gpt {
        return format!("{} {} {}", bus, size, l.s().no_table);
    }
    match l {
        Lang::En => format!("{} {} {} partition(s)", bus, size, partitions),
        Lang::Ru => format!("{} {} разделов: {}", bus, size, partitions),
    }
}

pub fn copied_line(l: Lang, done_mb: u64, total_mb: u64) -> String {
    match l {
        Lang::En => format!("{} of {} MB copied", done_mb, total_mb),
        Lang::Ru => format!("скопировано {} из {} МБ", done_mb, total_mb),
    }
}

/// A size as a person reads it, with the unit in their language
pub fn size(l: Lang, mb: u64) -> String {
    let (unit_tb, unit_gb, unit_mb) = match l {
        Lang::En => ("TB", "GB", "MB"),
        Lang::Ru => ("ТБ", "ГБ", "МБ"),
    };
    if mb >= 1024 * 1024 {
        format!("{}.{} {}", mb / (1024 * 1024), (mb % (1024 * 1024)) * 10 / (1024 * 1024), unit_tb)
    } else if mb >= 1024 {
        format!("{}.{} {}", mb / 1024, (mb % 1024) * 10 / 1024, unit_gb)
    } else {
        format!("{} {}", mb, unit_mb)
    }
}
