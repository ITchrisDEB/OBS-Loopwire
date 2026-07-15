// Widget de dock OBS pour loopwire. Uniquement la partie visuelle (Qt) ;
// toute la logique (pactl, mapping, config) reste en Rust côté lib.rs,
// appelée ici via les fonctions extern "C" ci-dessous. Aucune macro Q_OBJECT
// utilisée (connexions par lambda uniquement) : pas besoin de moc, juste une
// compilation C++ classique déclenchée depuis build.rs.

#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QLabel>
#include <QPushButton>
#include <QSlider>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QVBoxLayout>
#include <QWidget>
#include <Qt>

extern "C" {

struct FfiStatus {
	bool source_exists;
	bool mapped;
	bool muted;
	int volume_percent;
};

FfiStatus loopwire_get_status();
void loopwire_set_mute(bool muted);
void loopwire_set_volume(int percent);
char *loopwire_do_map();
char *loopwire_do_unmap();
void loopwire_free_string(char *s);
char *loopwire_get_config_card();
char *loopwire_get_config_source();
char *loopwire_get_config_sink();
bool loopwire_get_config_sink_auto();
char *loopwire_get_default_sink();
void loopwire_set_config(const char *card, const char *source, const char *sink, bool sink_auto);
char *loopwire_list_cards();
char *loopwire_list_sources();
char *loopwire_list_sinks();

} // extern "C"

static QStringList ffi_list_to_qstringlist(char *raw)
{
	QStringList out;
	if (raw) {
		out = QString::fromUtf8(raw).split('\n', Qt::SkipEmptyParts);
		loopwire_free_string(raw);
	}
	return out;
}

static QString ffi_string_take(char *raw)
{
	QString out;
	if (raw) {
		out = QString::fromUtf8(raw);
		loopwire_free_string(raw);
	}
	return out;
}

class ConfigDialog : public QDialog {
public:
	ConfigDialog(QWidget *parent) : QDialog(parent)
	{
		setWindowTitle("Configuration");

		// Carte/source : jamais de valeur figée dans le code — chaque machine
		// a un matériel de capture différent (ou aucun). La liste vient d'une
		// détection ponctuelle du système, faite une seule fois à l'ouverture
		// de ce dialogue (ou sur clic du bouton Refresh), jamais en boucle.
		cardBox = new QComboBox();
		cardBox->setEditable(true);

		sourceBox = new QComboBox();
		sourceBox->setEditable(true);

		sinkBox = new QComboBox();
		sinkBox->setEditable(true);

		// Sortie : un vrai concept de "sortie par défaut système" existe et a
		// du sens pour n'importe qui (contrairement à la carte de capture) —
		// donc case à cocher auto/manuel plutôt qu'un champ figé.
		sinkAutoBox = new QCheckBox();
		connect(sinkAutoBox, &QCheckBox::toggled, this, &ConfigDialog::onSinkAutoToggled);

		refreshButton = new QPushButton("🔄 Refresh detected list");
		connect(refreshButton, &QPushButton::clicked, this, [this]() { rescan(false); });

		auto *form = new QFormLayout();
		form->addRow("Device to enable/disable:", cardBox);
		form->addRow("Audio source to capture (input):", sourceBox);
		form->addRow("", sinkAutoBox);
		form->addRow("Manual audio output (speakers):", sinkBox);

		auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
		connect(buttons, &QDialogButtonBox::accepted, this, &QDialog::accept);
		connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);

		auto *layout = new QVBoxLayout(this);
		layout->addLayout(form);
		layout->addWidget(refreshButton);
		layout->addWidget(buttons);

		rescan(true);
	}

	void applyIfAccepted()
	{
		QByteArray card = cardBox->currentText().trimmed().toUtf8();
		QByteArray source = sourceBox->currentText().trimmed().toUtf8();
		QByteArray sink = sinkBox->currentText().trimmed().toUtf8();
		loopwire_set_config(card.constData(), source.constData(), sink.constData(),
					  sinkAutoBox->isChecked());
	}

private:
	QComboBox *cardBox;
	QComboBox *sourceBox;
	QComboBox *sinkBox;
	QCheckBox *sinkAutoBox;
	QPushButton *refreshButton;

	void onSinkAutoToggled(bool autoEnabled) { sinkBox->setEnabled(!autoEnabled); }

	// Re-scanne le système une seule fois (jamais en boucle), à la demande —
	// au premier affichage (loadFromConfig=true, pré-remplit depuis la
	// config enregistrée) ou sur clic du bouton Refresh (loadFromConfig=false,
	// garde ce que l'utilisateur a déjà sélectionné/tapé).
	void rescan(bool loadFromConfig)
	{
		QString card = loadFromConfig ? ffi_string_take(loopwire_get_config_card()) : cardBox->currentText();
		QString source =
			loadFromConfig ? ffi_string_take(loopwire_get_config_source()) : sourceBox->currentText();
		QString sink = loadFromConfig ? ffi_string_take(loopwire_get_config_sink()) : sinkBox->currentText();
		bool sinkAuto = loadFromConfig ? loopwire_get_config_sink_auto() : sinkAutoBox->isChecked();

		cardBox->clear();
		cardBox->addItems(ffi_list_to_qstringlist(loopwire_list_cards()));
		cardBox->setCurrentText(card);

		sourceBox->clear();
		sourceBox->addItems(ffi_list_to_qstringlist(loopwire_list_sources()));
		sourceBox->setCurrentText(source);

		sinkBox->clear();
		sinkBox->addItems(ffi_list_to_qstringlist(loopwire_list_sinks()));
		sinkBox->setCurrentText(sink);

		QString currentDefault = ffi_string_take(loopwire_get_default_sink());
		sinkAutoBox->setText(
			QString("System default output (%1)").arg(currentDefault.isEmpty() ? "none detected" : currentDefault));
		sinkAutoBox->setChecked(sinkAuto);
		sinkBox->setEnabled(!sinkAuto);
	}
};

class LoopWireDock : public QWidget {
public:
	LoopWireDock()
	{
		auto *layout = new QVBoxLayout(this);

		statusLabel = new QLabel("LoopWire");
		statusLabel->setAlignment(Qt::AlignCenter);

		muteButton = new QPushButton("🔊 Mute");
		connect(muteButton, &QPushButton::clicked, this, &LoopWireDock::onToggleMute);

		volumeSlider = new QSlider(Qt::Horizontal);
		volumeSlider->setRange(0, 150);
		connect(volumeSlider, &QSlider::valueChanged, this, &LoopWireDock::onVolumeChanged);

		volumeLabel = new QLabel("Volume: -- %");
		volumeLabel->setAlignment(Qt::AlignCenter);

		mappingLabel = new QLabel("Status: --");
		mappingLabel->setAlignment(Qt::AlignCenter);

		// Un seul bouton bascule (comme Mute juste au-dessus) plutôt que deux
		// boutons Map/Unmap séparés : coloré vert/rouge selon l'état, un clic
		// fait l'action opposée à l'état courant. Le label "Status" ci-dessus
		// reste affiché en plus (doublon visuel voulu).
		mapToggleButton = new QPushButton("🔗 Map");
		connect(mapToggleButton, &QPushButton::clicked, this, &LoopWireDock::onToggleMap);

		mapStatusLabel = new QLabel("");
		mapStatusLabel->setWordWrap(true);
		mapStatusLabel->setAlignment(Qt::AlignCenter);

		configButton = new QPushButton("⚙ Configuration");
		connect(configButton, &QPushButton::clicked, this, &LoopWireDock::onConfigure);

		layout->addWidget(statusLabel);
		layout->addWidget(muteButton);
		layout->addWidget(volumeSlider);
		layout->addWidget(volumeLabel);
		layout->addWidget(mappingLabel);
		layout->addWidget(mapToggleButton);
		layout->addWidget(mapStatusLabel);
		layout->addWidget(configButton);
		layout->addStretch();

		timer = new QTimer(this);
		connect(timer, &QTimer::timeout, this, &LoopWireDock::refresh);
		timer->start(2000);
		refresh();
	}

private:
	QLabel *statusLabel;
	QPushButton *muteButton;
	QSlider *volumeSlider;
	QLabel *volumeLabel;
	QLabel *mappingLabel;
	QPushButton *mapToggleButton;
	QLabel *mapStatusLabel;
	QPushButton *configButton;
	QTimer *timer;
	bool muted = false;
	bool mapped = false;
	bool updatingSlider = false;

	void refresh()
	{
		FfiStatus st = loopwire_get_status();
		mappingLabel->setText(st.mapped ? "Status: ✅ Mapped" : "Status: ❌ Unmapped");

		mapped = st.mapped;
		if (mapped) {
			mapToggleButton->setText("✅ Mapped (click to unmap)");
			mapToggleButton->setStyleSheet("background-color: #2e7d32; color: white;");
		} else {
			mapToggleButton->setText("❌ Unmapped (click to map)");
			mapToggleButton->setStyleSheet("background-color: #c62828; color: white;");
		}

		if (!st.source_exists) {
			statusLabel->setText("LoopWire — not found");
			muteButton->setEnabled(false);
			volumeSlider->setEnabled(false);
			volumeLabel->setText("Volume: -- %");
			return;
		}

		statusLabel->setText("LoopWire");
		muteButton->setEnabled(true);
		volumeSlider->setEnabled(true);
		muted = st.muted;
		muteButton->setText(muted ? "🔇 Unmute" : "🔊 Mute");

		updatingSlider = true;
		volumeSlider->setValue(st.volume_percent);
		updatingSlider = false;
		volumeLabel->setText(QString("Volume: %1%").arg(st.volume_percent));
	}

	void onToggleMute()
	{
		loopwire_set_mute(!muted);
		refresh();
	}

	void onVolumeChanged(int value)
	{
		if (updatingSlider)
			return;
		loopwire_set_volume(value);
		volumeLabel->setText(QString("Volume: %1%").arg(value));
	}

	// Un seul clic -> l'action opposée à l'état courant (comme le bouton
	// Mute) : jamais de minuteur, jamais répété automatiquement.
	void onToggleMap()
	{
		mapToggleButton->setEnabled(false);
		QString status;
		if (mapped) {
			status = ffi_string_take(loopwire_do_unmap());
		} else {
			mapStatusLabel->setText("Mapping…");
			status = ffi_string_take(loopwire_do_map());
		}
		mapStatusLabel->setText(status);
		mapToggleButton->setEnabled(true);
		refresh();
	}

	void onConfigure()
	{
		ConfigDialog dialog(this);
		if (dialog.exec() == QDialog::Accepted) {
			dialog.applyIfAccepted();
			mapStatusLabel->setText("Configuration saved.");
			refresh();
		}
	}
};

extern "C" void *loopwire_create_dock_widget()
{
	return new LoopWireDock();
}
