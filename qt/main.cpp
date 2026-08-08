// SwiftInvert's Linux shell: Qt Widgets over the SwiftInvertCore C bridge.
// Deliberately thin — decode/analysis/render/settings semantics all live in
// the Swift core; this file is folder browsing, a canvas, controls that edit
// a settings JSON object, sidecar load/save, and latest-wins async rendering.
//
// The control set mirrors the Mac's ControlsSidebar exactly: same sections,
// same ranges/defaults, same direction conventions (Brightness is 2 − density
// so right = brighter print). When the Mac sidebar gains a control, add it
// here with the same spec.

#include <QtConcurrent/QtConcurrent>
#include <QtWidgets>

#include "swiftinvert_core.h"

namespace {

QString takeString(char *owned) {
    QString s = QString::fromUtf8(owned ? owned : "");
    si_free(owned);
    return s;
}

const QStringList kRawPatterns = {
    "*.nef", "*.cr2", "*.cr3", "*.arw", "*.raf", "*.dng",
    "*.orf", "*.rw2", "*.pef", "*.srw", "*.iiq", "*.3fr",
};

struct RenderOutcome {
    QImage image;
    QVector<quint32> histogram;  // 1024 bins: R,G,B,luma × 256
    qint64 milliseconds = 0;
};

}  // namespace

// ── Canvas ────────────────────────────────────────────────────────────────

class ImageView : public QWidget {
public:
    explicit ImageView(QWidget *parent = nullptr) : QWidget(parent) {
        setMinimumSize(400, 300);
        QPalette pal = palette();
        pal.setColor(QPalette::Window, QColor(32, 32, 34));
        setAutoFillBackground(true);
        setPalette(pal);
    }
    void setImage(const QImage &image) {
        image_ = image;
        update();
    }

protected:
    void paintEvent(QPaintEvent *) override {
        QPainter p(this);
        if (image_.isNull()) {
            p.setPen(QColor(150, 150, 150));
            p.drawText(rect(), Qt::AlignCenter, tr("Open a folder and pick a frame"));
            return;
        }
        QSize target = image_.size().scaled(size(), Qt::KeepAspectRatio);
        QRect dst(QPoint((width() - target.width()) / 2, (height() - target.height()) / 2), target);
        p.setRenderHint(QPainter::SmoothPixmapTransform);
        p.drawImage(dst, image_);
    }

private:
    QImage image_;
};

// ── Histogram (mirrors HistogramView: peak-normalized, fixed log
//    compression so the curve is a property of the shape alone) ────────────

class HistogramWidget : public QWidget {
public:
    explicit HistogramWidget(QWidget *parent = nullptr) : QWidget(parent) {
        setFixedHeight(92);
        setMinimumWidth(200);
    }
    void setBins(const QVector<quint32> &bins) {
        bins_ = bins;
        update();
    }

protected:
    void paintEvent(QPaintEvent *) override {
        QPainter p(this);
        p.fillRect(rect(), QColor(24, 24, 26));
        p.setPen(QColor(60, 60, 64));
        for (int q = 1; q < 4; ++q) {
            const int x = width() * q / 4;
            p.drawLine(x, 0, x, height());
        }
        if (bins_.size() < 1024) return;
        quint32 maxCount = 1;
        for (int i = 0; i < 1024; ++i) maxCount = qMax(maxCount, bins_[i]);

        const double kLogCompression = 100000.0;  // HistogramView.logCompression
        auto barHeight = [&](quint32 count) {
            const double f = double(count) / double(maxCount);
            return std::log1p(f * kLogCompression) / std::log1p(kLogCompression);
        };
        auto channelPath = [&](int channel) {
            QPainterPath path(QPointF(0, height()));
            for (int i = 0; i < 256; ++i) {
                const double y = height() * (1.0 - barHeight(bins_[channel * 256 + i]));
                path.lineTo(width() * double(i) / 255.0, y);
            }
            path.lineTo(width(), height());
            path.closeSubpath();
            return path;
        };
        p.setRenderHint(QPainter::Antialiasing);
        const QColor fills[3] = {
            QColor(235, 80, 80, 90), QColor(90, 220, 90, 90), QColor(90, 120, 245, 90)};
        const QColor strokes[3] = {
            QColor(235, 80, 80, 180), QColor(90, 220, 90, 180), QColor(90, 120, 245, 180)};
        for (int ch = 0; ch < 3; ++ch) {
            QPainterPath path = channelPath(ch);
            p.fillPath(path, fills[ch]);
            p.strokePath(path, QPen(strokes[ch], 1));
        }
        QPainterPath luma(QPointF(0, height()));
        for (int i = 0; i < 256; ++i)
            luma.lineTo(width() * double(i) / 255.0,
                        height() * (1.0 - barHeight(bins_[768 + i])));
        p.strokePath(luma, QPen(QColor(230, 230, 230, 200), 1));
    }

private:
    QVector<quint32> bins_;
};

// ── Main window ───────────────────────────────────────────────────────────

class MainWindow : public QMainWindow {
public:
    MainWindow() {
        defaults_ = QJsonDocument::fromJson(takeString(si_default_settings()).toUtf8()).object();
        settings_ = defaults_;

        saveTimer_ = new QTimer(this);
        saveTimer_->setSingleShot(true);
        saveTimer_->setInterval(1000);  // Mac: debounced 1 s sidecar save
        connect(saveTimer_, &QTimer::timeout, this, &MainWindow::flushSidecar);

        auto *splitter = new QSplitter(this);
        splitter->addWidget(makeLibraryPanel());
        canvas_ = new ImageView;
        splitter->addWidget(canvas_);
        splitter->addWidget(makeControlsPanel());
        splitter->setStretchFactor(0, 0);
        splitter->setStretchFactor(1, 1);
        splitter->setStretchFactor(2, 0);
        setCentralWidget(splitter);
        setWindowTitle(tr("SwiftInvert"));
        resize(1500, 940);
        statusBar()->showMessage(tr("Ready"));

        auto *openAction = new QAction(tr("&Open Folder…"), this);
        openAction->setShortcut(QKeySequence::Open);
        connect(openAction, &QAction::triggered, this, &MainWindow::chooseFolder);
        menuBar()->addMenu(tr("&File"))->addAction(openAction);
    }

    ~MainWindow() override {
        flushSidecar();
        if (session_) si_close(session_);
    }

    void openFolder(const QString &path) {
        fileList_->clear();
        const QDir dir(path);
        const QFileInfoList files = dir.entryInfoList(kRawPatterns, QDir::Files, QDir::Name);
        for (const QFileInfo &info : files) {
            auto *item = new QListWidgetItem(info.fileName());
            item->setData(Qt::UserRole, info.absoluteFilePath());
            fileList_->addItem(item);
        }
        statusBar()->showMessage(tr("%1 frames").arg(files.size()));
        loadThumbnailsAsync();
        if (fileList_->count() > 0) fileList_->setCurrentRow(0);
    }

    void selectFile(const QString &path) {
        flushSidecar();  // pending edits belong to the frame being left
        currentPath_ = path;
        statusBar()->showMessage(tr("Opening %1…").arg(QFileInfo(path).fileName()));
        openGeneration_ += 1;
        const quint64 generation = openGeneration_;
        auto *watcher = new QFutureWatcher<int64_t>(this);
        connect(watcher, &QFutureWatcher<int64_t>::finished, this, [this, watcher, generation, path] {
            watcher->deleteLater();
            const int64_t handle = watcher->result();
            if (generation != openGeneration_) {  // stale open: user moved on
                if (handle) si_close(handle);
                return;
            }
            if (!handle) {
                statusBar()->showMessage(tr("Failed: %1").arg(takeString(si_last_error())));
                return;
            }
            if (session_) si_close(session_);
            session_ = handle;
            loadSidecar(path);
            updateInfoRow();
            statusBar()->showMessage(QFileInfo(path).fileName());
            requestRender();
        });
        watcher->setFuture(QtConcurrent::run([path] {
            return si_open(path.toUtf8().constData());
        }));
    }

    // Latest-wins render queue: at most one in flight, newest request replaces
    // any queued one (the same coalescing discipline as the Mac app).
    void requestRender() {
        if (!session_) return;
        if (renderInFlight_) {
            renderQueued_ = true;
            return;
        }
        renderInFlight_ = true;
        const int64_t session = session_;
        const QByteArray json =
            QJsonDocument(showingBaseline_ ? QJsonObject() : settings_)
                .toJson(QJsonDocument::Compact);
        auto *watcher = new QFutureWatcher<RenderOutcome>(this);
        connect(watcher, &QFutureWatcher<RenderOutcome>::finished, this, [this, watcher] {
            watcher->deleteLater();
            renderInFlight_ = false;
            const RenderOutcome outcome = watcher->result();
            if (!outcome.image.isNull()) {
                canvas_->setImage(outcome.image);
                histogram_->setBins(outcome.histogram);
                statusBar()->showMessage(
                    tr("render %1 ms — %2").arg(outcome.milliseconds).arg(deviceName()));
            } else {
                statusBar()->showMessage(tr("Render failed: %1").arg(takeString(si_last_error())));
            }
            if (renderQueued_) {
                renderQueued_ = false;
                requestRender();
            }
        });
        watcher->setFuture(QtConcurrent::run([session, json] {
            QElapsedTimer timer;
            timer.start();
            RenderOutcome outcome;
            outcome.histogram.resize(1024);
            int32_t w = 0, h = 0;
            uint8_t *rgba = si_render(session, json.constData(), /*srgb_display=*/1, &w, &h,
                                      outcome.histogram.data());
            outcome.milliseconds = timer.elapsed();
            if (rgba) {
                outcome.image = QImage(rgba, w, h, w * 4, QImage::Format_RGBA8888).copy();
                si_free(rgba);
            }
            return outcome;
        }));
    }

    bool selfTest(const QString &target, const QString &screenshotPath);

protected:
    void closeEvent(QCloseEvent *event) override {
        flushSidecar();
        QMainWindow::closeEvent(event);
    }

private:
    // ── Settings model ────────────────────────────────────────────────────

    double settingValue(const QString &key) const { return settings_.value(key).toDouble(); }
    bool settingBool(const QString &key) const { return settings_.value(key).toBool(); }

    void editSetting(const QString &key, const QJsonValue &value) {
        settings_.insert(key, value);
        markDirtyAndRender();
    }

    double gradingValue(const QString &key, int component) const {
        const QJsonArray a = settings_.value(key).toArray();
        return component < a.size() ? a[component].toDouble() : 0.0;
    }

    void editGrading(const QString &key, int component, double v) {
        QJsonArray a = settings_.value(key).toArray();
        while (a.size() < 3) a.append(0.0);
        a[component] = v;
        settings_.insert(key, a);
        markDirtyAndRender();
    }

    void markDirtyAndRender() {
        if (!restoring_) {
            sidecarDirty_ = true;
            saveTimer_->start();
            refreshDependentStates();
        }
        requestRender();
    }

    void loadSidecar(const QString &path) {
        restoring_ = true;
        settings_ = defaults_;
        if (char *json = si_sidecar_load(path.toUtf8().constData())) {
            const QJsonObject loaded = QJsonDocument::fromJson(takeString(json).toUtf8()).object();
            for (auto it = loaded.begin(); it != loaded.end(); ++it)
                settings_.insert(it.key(), it.value());
        }
        sidecarDirty_ = false;
        refreshAllControls();
        restoring_ = false;
    }

    void flushSidecar() {
        saveTimer_->stop();
        if (!sidecarDirty_ || currentPath_.isEmpty()) return;
        const QByteArray json = QJsonDocument(settings_).toJson(QJsonDocument::Compact);
        if (si_sidecar_save(currentPath_.toUtf8().constData(), json.constData()))
            sidecarDirty_ = false;
        else
            statusBar()->showMessage(tr("Sidecar save failed: %1").arg(takeString(si_last_error())));
    }

    void resetAll() {
        settings_ = defaults_;
        refreshAllControls();
        markDirtyAndRender();
    }

    void refreshAllControls() {
        for (const auto &refresh : refreshers_) refresh();
        refreshDependentStates();
    }

    void refreshDependentStates() {
        // Mac: Separation Damping is inert (disabled) while Print Saturation
        // is exactly 1.
        if (separationDampingRow_)
            separationDampingRow_->setEnabled(settingValue("printSaturation") != 1.0);
    }

    // ── Control builders (spec mirrors ControlsSidebar) ───────────────────

    // One slider row: caption + reset-⨯ (visible when off-default) + value
    // label; double-click resets. `get`/`set` bridge UI value ↔ settings.
    QWidget *sliderRow(const QString &label, double minimum, double maximum, double step,
                      double defaultValue, int decimals, const QString &suffix,
                      std::function<double()> get, std::function<void(double)> set) {
        auto *row = new QWidget;
        auto *grid = new QGridLayout(row);
        grid->setContentsMargins(0, 0, 0, 0);
        grid->setVerticalSpacing(0);

        auto *caption = new QLabel(label);
        caption->setStyleSheet("font-size: 11px;");
        auto *reset = new QToolButton;
        reset->setText("✕");
        reset->setStyleSheet("QToolButton { border: none; font-size: 9px; color: gray; }");
        reset->setCursor(Qt::PointingHandCursor);
        auto *value = new QLabel;
        value->setStyleSheet("font-size: 11px;");
        value->setAlignment(Qt::AlignRight | Qt::AlignVCenter);

        auto *slider = new QSlider(Qt::Horizontal);
        const int ticks = int((maximum - minimum) / step + 0.5);
        slider->setRange(0, ticks);

        auto toTicks = [minimum, step](double v) { return int((v - minimum) / step + 0.5); };
        auto fromTicks = [minimum, step](int t) { return minimum + t * step; };
        auto updateLabels = [value, reset, defaultValue, decimals, suffix](double v) {
            value->setText(QString::number(v, 'f', decimals) + suffix);
            const bool changed = std::abs(v - defaultValue) > 1e-9;
            reset->setVisible(changed);
            value->setStyleSheet(changed ? "font-size: 11px;"
                                         : "font-size: 11px; color: gray;");
        };

        connect(slider, &QSlider::valueChanged, this, [this, set, fromTicks, updateLabels](int t) {
            const double v = fromTicks(t);
            updateLabels(v);
            if (!refreshing_) set(v);
        });
        connect(reset, &QToolButton::clicked, this,
                [slider, toTicks, defaultValue] { slider->setValue(toTicks(defaultValue)); });
        slider->installEventFilter(this);
        sliderDefaults_.insert(slider, toTicks(defaultValue));

        refreshers_.push_back([this, slider, get, toTicks, updateLabels] {
            refreshing_ = true;
            QSignalBlocker block(slider);
            const double v = get();
            slider->setValue(toTicks(v));
            updateLabels(v);
            refreshing_ = false;
        });

        grid->addWidget(caption, 0, 0);
        grid->addWidget(reset, 0, 1);
        grid->addWidget(value, 0, 2);
        grid->addWidget(slider, 1, 0, 1, 3);
        const double initial = get();
        {
            QSignalBlocker block(slider);
            slider->setValue(toTicks(initial));
        }
        updateLabels(initial);
        return row;
    }

    QWidget *settingSlider(const QString &label, const QString &key, double minimum,
                           double maximum, double step, double defaultValue, int decimals = 2,
                           const QString &suffix = QString()) {
        return sliderRow(
            label, minimum, maximum, step, defaultValue, decimals, suffix,
            [this, key] { return settingValue(key); },
            [this, key](double v) { editSetting(key, v); });
    }

    QCheckBox *settingToggle(const QString &label, const QString &key) {
        auto *box = new QCheckBox(label);
        box->setChecked(settingBool(key));
        connect(box, &QCheckBox::toggled, this, [this, key](bool on) {
            if (!refreshing_) editSetting(key, on);
        });
        refreshers_.push_back([this, box, key] {
            refreshing_ = true;
            QSignalBlocker block(box);
            box->setChecked(settingBool(key));
            refreshing_ = false;
        });
        return box;
    }

    QWidget *bandPicker(const QStringList &names, std::function<void(int)> onChange) {
        auto *row = new QWidget;
        auto *layout = new QHBoxLayout(row);
        layout->setContentsMargins(0, 0, 0, 0);
        layout->setSpacing(0);
        auto *group = new QButtonGroup(row);
        group->setExclusive(true);
        for (int i = 0; i < names.size(); ++i) {
            auto *b = new QPushButton(names[i]);
            b->setCheckable(true);
            b->setChecked(i == 0);
            b->setStyleSheet("font-size: 11px; padding: 2px;");
            group->addButton(b, i);
            layout->addWidget(b);
        }
        connect(group, &QButtonGroup::idClicked, this, std::move(onChange));
        return row;
    }

    static QGroupBox *section(const QString &title, std::initializer_list<QWidget *> rows) {
        auto *box = new QGroupBox(title);
        auto *layout = new QVBoxLayout(box);
        layout->setSpacing(8);
        for (QWidget *w : rows) layout->addWidget(w);
        return box;
    }

    QWidget *makeControlsPanel() {
        auto *panel = new QWidget;
        auto *layout = new QVBoxLayout(panel);
        layout->setSpacing(12);

        // Header: title + Reset All (matches the sidebar header).
        {
            auto *header = new QWidget;
            auto *h = new QHBoxLayout(header);
            h->setContentsMargins(0, 0, 0, 0);
            auto *title = new QLabel(tr("<b>Adjustments</b>"));
            auto *reset = new QPushButton(tr("Reset All"));
            connect(reset, &QPushButton::clicked, this, &MainWindow::resetAll);
            h->addWidget(title);
            h->addStretch();
            h->addWidget(reset);
            layout->addWidget(header);
        }

        histogram_ = new HistogramWidget;
        layout->addWidget(histogram_);

        // Pre-process: hold-to-compare against the stock conversion.
        auto *viewOriginal = new QPushButton(tr("View Original (hold)"));
        connect(viewOriginal, &QPushButton::pressed, this, [this] {
            showingBaseline_ = true;
            requestRender();
        });
        connect(viewOriginal, &QPushButton::released, this, [this] {
            showingBaseline_ = false;
            requestRender();
        });
        layout->addWidget(section(tr("Pre-process"), {viewOriginal}));

        // Print. Brightness: right = brighter; internally density = 2 − b.
        infoRow_ = new QLabel;
        infoRow_->setStyleSheet("font-size: 10px; color: gray;");
        layout->addWidget(section(
            tr("Print"),
            {
                settingToggle(tr("Auto exposure"), "autoExposure"),
                sliderRow(
                    tr("Brightness"), -3, 4, 0.01, 1.0, 2, QString(),
                    [this] { return 2.0 - settingValue("density"); },
                    [this](double b) { editSetting("density", 2.0 - b); }),
                settingToggle(tr("Auto contrast"), "autoNormalizeContrast"),
                settingSlider(tr("Grade (ISO R)"), "grade", 50, 180, 1, 115, 0),
                infoRow_,
            }));

        layout->addWidget(section(
            tr("Exposure"),
            {
                settingSlider(tr("Exposure (stops)"), "exposureStops", -2, 2, 0.01, 0),
                settingSlider(tr("Contrast"), "overallContrast", -1, 2, 0.01, 0),
                settingSlider(tr("Shadows"), "shadows", -2, 2, 0.01, 0),
                settingSlider(tr("Shadow contrast"), "shadowContrast", -3, 6, 0.01, 0),
                settingSlider(tr("Dark shadows"), "darkShadows", -2, 2, 0.01, 0),
                settingSlider(tr("Highlights"), "highlights", -1, 1, 0.01, 0),
                settingSlider(tr("Highlight contrast"), "highlightContrast", -1, 1, 0.01, 0),
            }));

        separationDampingRow_ =
            settingSlider(tr("Separation Damping"), "separationDamping", 0, 1, 0.01, 0);
        layout->addWidget(section(
            tr("Color"),
            {
                settingSlider(tr("Pre-saturation"), "preSaturation", 0.5, 2.0, 0.01, 1.15),
                settingSlider(tr("Cast strength"), "castRemovalStrength", 0, 2, 0.01, 0.5),
                settingSlider(tr("Hue Trim"), "hueTrim", -30, 30, 0.1, 0, 1, QStringLiteral("°")),
                settingSlider(tr("Print Saturation"), "printSaturation", 0, 2, 0.01, 1.0),
                separationDampingRow_,
                settingSlider(tr("Vibrance"), "vibrance", 0, 2, 0.01, 1.0),
                settingSlider(tr("Saturation"), "saturation", 0, 2, 0.01, 1.0),
                settingSlider(tr("Skin Protection"), "skinProtection", 0, 1, 0.01, 0.5),
            }));

        // Color Mixer: R/Y/G/B band picker + Hue/Saturation for the band.
        {
            static const char *hueKeys[4] = {"redHue", "yellowHue", "greenHue", "blueHue"};
            static const char *satKeys[4] = {"redSaturation", "yellowSaturation",
                                             "greenSaturation", "blueSaturation"};
            auto *picker = bandPicker({tr("R"), tr("Y"), tr("G"), tr("B")}, [this](int band) {
                mixerBand_ = band;
                refreshAllControls();
            });
            layout->addWidget(section(
                tr("Color Mixer"),
                {
                    picker,
                    sliderRow(
                        tr("Hue"), -1.5, 1.5, 0.01, 0, 2, QString(),
                        [this] { return settingValue(hueKeys[mixerBand_]); },
                        [this](double v) { editSetting(hueKeys[mixerBand_], v); }),
                    sliderRow(
                        tr("Saturation"), 0, 2, 0.01, 1.0, 2, QString(),
                        [this] { return settingValue(satKeys[mixerBand_]); },
                        [this](double v) { editSetting(satKeys[mixerBand_], v); }),
                }));
        }

        // Color Grading: Temp/Tint + per-band R↔C / G↔M / B↔Y.
        {
            static const char *bandKeys[3] = {"colorShadows", "colorMids", "colorHighs"};
            auto *picker =
                bandPicker({tr("Shadows"), tr("Mids"), tr("Highs")}, [this](int band) {
                    gradingBand_ = band;
                    refreshAllControls();
                });
            auto channelSlider = [this](const QString &label, int component) {
                return sliderRow(
                    label, -1, 1, 0.01, 0, 2, QString(),
                    [this, component] { return gradingValue(bandKeys[gradingBand_], component); },
                    [this, component](double v) { editGrading(bandKeys[gradingBand_], component, v); });
            };
            layout->addWidget(section(
                tr("Color Grading"),
                {
                    settingSlider(tr("Temp"), "temp", -1, 1, 0.01, 0),
                    settingSlider(tr("Tint"), "tint", -1, 1, 0.01, 0),
                    picker,
                    channelSlider(tr("R ↔ C"), 0),
                    channelSlider(tr("G ↔ M"), 1),
                    channelSlider(tr("B ↔ Y"), 2),
                }));
        }
        // Grading band starts on Mids, matching the Mac.
        gradingBand_ = 1;

        layout->addWidget(section(
            tr("Tone"),
            {
                settingSlider(tr("Toe"), "toe", -1, 1, 0.01, 0),
                settingSlider(tr("Shoulder"), "shoulder", -1, 1, 0.01, 0),
                settingToggle(tr("True black"), "trueBlack"),
                settingSlider(tr("White point"), "whitePointOffset", -0.3, 0.3, 0.001, 0, 3),
                settingSlider(tr("Black point"), "blackPointOffset", -0.3, 0.3, 0.001, 0, 3),
            }));

        layout->addStretch();
        refreshDependentStates();

        auto *scroll = new QScrollArea;
        scroll->setWidget(panel);
        scroll->setWidgetResizable(true);
        scroll->setMinimumWidth(340);
        scroll->setMaximumWidth(400);
        scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
        return scroll;
    }

    // ── Library ───────────────────────────────────────────────────────────

    QString deviceName() {
        if (cachedDevice_.isEmpty()) cachedDevice_ = takeString(si_device_name());
        return cachedDevice_;
    }

    void chooseFolder() {
        const QString path = QFileDialog::getExistingDirectory(this, tr("Open Folder"));
        if (!path.isEmpty()) openFolder(path);
    }

    QWidget *makeLibraryPanel() {
        fileList_ = new QListWidget;
        fileList_->setIconSize(QSize(148, 112));
        fileList_->setMinimumWidth(230);
        fileList_->setMaximumWidth(300);
        fileList_->setSpacing(2);
        connect(fileList_, &QListWidget::currentItemChanged, this,
                [this](QListWidgetItem *item, QListWidgetItem *) {
                    if (item) selectFile(item->data(Qt::UserRole).toString());
                });
        return fileList_;
    }

    void loadThumbnailsAsync() {
        QStringList paths;
        for (int i = 0; i < fileList_->count(); ++i)
            paths.append(fileList_->item(i)->data(Qt::UserRole).toString());
        thumbGeneration_ += 1;
        const quint64 generation = thumbGeneration_;
        auto *self = this;
        (void)QtConcurrent::run([self, paths, generation] {
            for (int i = 0; i < paths.size(); ++i) {
                int32_t length = 0;
                uint8_t *jpeg = si_thumbnail(paths[i].toUtf8().constData(), &length);
                if (!jpeg) continue;
                QImage image = QImage::fromData(jpeg, length);
                si_free(jpeg);
                if (image.isNull()) continue;
                const QIcon icon(QPixmap::fromImage(
                    image.scaled(148, 112, Qt::KeepAspectRatio, Qt::SmoothTransformation)));
                QMetaObject::invokeMethod(self, [self, i, icon, generation] {
                    if (generation != self->thumbGeneration_) return;
                    if (i < self->fileList_->count()) self->fileList_->item(i)->setIcon(icon);
                }, Qt::QueuedConnection);
            }
        });
    }

    void updateInfoRow() {
        // Negative character (the row under Grade on the Mac): measured
        // density range vs the grade default, plus cast confidence.
        const QString json = takeString(si_session_info(session_));
        const QJsonObject info = QJsonDocument::fromJson(json.toUtf8()).object();
        if (info.isEmpty()) {
            infoRow_->clear();
            return;
        }
        QString text = tr("negative: %1").arg(info.value("densityRange").toDouble(), 0, 'f', 2);
        if (info.contains("character"))
            text += QStringLiteral(" · %1").arg(info.value("character").toString());
        if (info.contains("castConfidence"))
            text += tr("   cast confidence %1")
                        .arg(info.value("castConfidence").toDouble(), 0, 'f', 2);
        infoRow_->setText(text);
    }

    bool eventFilter(QObject *object, QEvent *event) override {
        // Double-click any slider = reset to default (LabeledSlider parity).
        if (event->type() == QEvent::MouseButtonDblClick) {
            if (auto *slider = qobject_cast<QSlider *>(object)) {
                const auto it = sliderDefaults_.constFind(slider);
                if (it != sliderDefaults_.constEnd()) {
                    slider->setValue(it.value());
                    return true;
                }
            }
        }
        return QMainWindow::eventFilter(object, event);
    }

    // ── State ─────────────────────────────────────────────────────────────

    QListWidget *fileList_ = nullptr;
    ImageView *canvas_ = nullptr;
    HistogramWidget *histogram_ = nullptr;
    QLabel *infoRow_ = nullptr;
    QWidget *separationDampingRow_ = nullptr;
    QJsonObject defaults_;
    QJsonObject settings_;
    std::vector<std::function<void()>> refreshers_;
    QHash<QSlider *, int> sliderDefaults_;
    QTimer *saveTimer_ = nullptr;
    QString currentPath_;
    QString cachedDevice_;
    int64_t session_ = 0;
    int mixerBand_ = 0;
    int gradingBand_ = 1;
    quint64 openGeneration_ = 0;
    bool renderInFlight_ = false;
    bool renderQueued_ = false;
    bool restoring_ = false;
    bool refreshing_ = false;
    bool sidecarDirty_ = false;
    bool showingBaseline_ = false;

public:
    quint64 thumbGeneration_ = 0;
};

// Headless proof: open a file (or a folder's first RAW), render synchronously
// through the same code paths, screenshot the whole window. Run with
// QT_QPA_PLATFORM=offscreen for CI-style checks.
bool MainWindow::selfTest(const QString &target, const QString &screenshotPath) {
    QFileInfo info(target);
    QString file = target;
    if (info.isDir()) {
        openFolder(target);
        if (fileList_->count() == 0) return false;
        file = fileList_->item(0)->data(Qt::UserRole).toString();
    } else {
        openFolder(info.absolutePath());
    }
    const int64_t handle = si_open(file.toUtf8().constData());
    if (!handle) {
        fprintf(stderr, "selftest: %s\n", qPrintable(takeString(si_last_error())));
        return false;
    }
    if (session_) si_close(session_);
    session_ = handle;
    currentPath_ = file;
    loadSidecar(file);
    updateInfoRow();
    int32_t w = 0, h = 0;
    QVector<quint32> bins(1024);
    const QByteArray json = QJsonDocument(settings_).toJson(QJsonDocument::Compact);
    uint8_t *rgba = si_render(session_, json.constData(), 1, &w, &h, bins.data());
    if (!rgba) {
        fprintf(stderr, "selftest: %s\n", qPrintable(takeString(si_last_error())));
        return false;
    }
    canvas_->setImage(QImage(rgba, w, h, w * 4, QImage::Format_RGBA8888).copy());
    si_free(rgba);
    histogram_->setBins(bins);
    statusBar()->showMessage(tr("selftest — %1x%2 via %3").arg(w).arg(h).arg(deviceName()));
    show();
    // Let thumbnails land so the screenshot shows the library too.
    QDeadlineTimer deadline(4000);
    while (!deadline.hasExpired()) {
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
        QThread::msleep(20);
        bool allIconsSet = fileList_->count() > 0;
        for (int i = 0; i < fileList_->count(); ++i)
            if (fileList_->item(i)->icon().isNull()) allIconsSet = false;
        if (allIconsSet) break;
    }
    QCoreApplication::processEvents();
    return grab().save(screenshotPath);
}

int main(int argc, char **argv) {
    QApplication app(argc, argv);
    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addPositionalArgument("folder", "Folder (or RAW file) to open");
    QCommandLineOption selfTestOption("selftest", "Render + screenshot to <png>, then exit", "png");
    parser.addOption(selfTestOption);
    parser.process(app);

    MainWindow window;
    const QStringList args = parser.positionalArguments();
    if (parser.isSet(selfTestOption)) {
        if (args.isEmpty()) {
            fprintf(stderr, "selftest needs a folder/file argument\n");
            return 2;
        }
        return window.selfTest(args.first(), parser.value(selfTestOption)) ? 0 : 1;
    }
    window.show();
    if (!args.isEmpty()) window.openFolder(args.first());
    return app.exec();
}
