// SwiftInvert's Linux shell: Qt Widgets over the SwiftInvertCore C bridge.
// Deliberately thin — decode/analysis/render/settings semantics all live in
// the Swift core; this file is folder browsing, a canvas with crop/analysis
// tools, controls that edit a settings JSON object, per-file history,
// sidecar load/save, and latest-wins async rendering.
//
// The control set mirrors the Mac's ControlsSidebar exactly: same sections,
// same ranges/defaults, same direction conventions (Brightness is 2 − density
// so right = brighter print). When the Mac sidebar gains a control, add it
// here with the same spec.
//
// Geometry conventions (must match the bridge Session / Mac ImageSession):
// cropRect is normalized on the FINE-ROTATED inscribed frame — so the crop
// tool renders uncropped at the current angle and the drawn box IS the
// stored rect. The analysis tool renders the orientation-only frame
// (no fine rotation, no crop), so its rect maps 1:1 to the metering space.

#include <QtConcurrent/QtConcurrent>
#include <QtWidgets>

#include "imageview.h"
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

QJsonObject rectToJson(const QRectF &r) {
    return {{"x", r.x()}, {"y", r.y()}, {"width", r.width()}, {"height", r.height()}};
}

// Gradient track for a slider, mirroring the Mac's GradientSlider: the
// groove carries the control's color meaning (2 or 3 stops), the handle
// stays neutral. Colors are the Mac sections' exact literals.
QString gradientStyle(const QStringList &stops) {
    QString bg = "qlineargradient(x1:0, y1:0, x2:1, y2:0";
    for (int i = 0; i < stops.size(); ++i)
        bg += QString(", stop:%1 %2").arg(double(i) / (stops.size() - 1)).arg(stops[i]);
    bg += ")";
    return QString(
        "QSlider::groove:horizontal { height: 6px; border-radius: 3px; background: %1; }"
        "QSlider::handle:horizontal { width: 12px; margin: -4px 0; border-radius: 6px;"
        " background: #EEEEEE; border: 1px solid #999999; }")
        .arg(bg);
}

QRectF rectFromJson(const QJsonValue &v) {
    if (!v.isObject()) return {};
    const QJsonObject o = v.toObject();
    return QRectF(o.value("x").toDouble(), o.value("y").toDouble(),
                  o.value("width").toDouble(), o.value("height").toDouble());
}

}  // namespace

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
    enum class Tool { None, Crop, Analysis };

    MainWindow() {
        defaults_ = QJsonDocument::fromJson(takeString(si_default_settings()).toUtf8()).object();
        settings_ = defaults_;

        saveTimer_ = new QTimer(this);
        saveTimer_->setSingleShot(true);
        saveTimer_->setInterval(1000);  // Mac: debounced 1 s sidecar save
        connect(saveTimer_, &QTimer::timeout, this, &MainWindow::flushSidecar);

        makeToolBar();

        auto *canvasColumn = new QWidget;
        auto *canvasLayout = new QVBoxLayout(canvasColumn);
        canvasLayout->setContentsMargins(0, 0, 0, 0);
        canvasLayout->setSpacing(0);
        canvasLayout->addWidget(makeCropBar());
        canvas_ = new ImageView;
        canvas_->onBoxCommitted = [this] { boxCommitted(); };
        canvas_->onZoomChanged = [this](double z) {
            canvasZoom_ = z;
            if (desiredTier() != lastTier_) requestRender();  // threshold crossing
        };
        canvasLayout->addWidget(canvas_, 1);
        canvasLayout->addWidget(makeCanvasBar());

        auto *splitter = new QSplitter(this);
        splitter->addWidget(makeLibraryPanel());
        splitter->addWidget(canvasColumn);
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

        auto *escape = new QShortcut(QKeySequence(Qt::Key_Escape), this);
        connect(escape, &QShortcut::activated, this, [this] { setTool(Tool::None, false); });
    }

    ~MainWindow() override {
        alive_->store(false);
        flushSidecar();
        if (session_) si_close(session_);
    }

    // Recursive scan with the Mac's rules (AppModel.buildTree/flatten):
    // depth-capped at 8, hidden entries skipped, natural sort, and a
    // folder's own files before its subfolders — the filmstrip order
    // behind index numbers there and here.
    static void scanFolder(const QString &path, const QString &rootPath, int depth,
                           QStringList &out) {
        if (depth > 8) return;
        QDir dir(path);
        QCollator collator;
        collator.setNumericMode(true);
        QFileInfoList files = dir.entryInfoList(kRawPatterns, QDir::Files);
        std::sort(files.begin(), files.end(), [&](const QFileInfo &a, const QFileInfo &b) {
            return collator.compare(a.fileName(), b.fileName()) < 0;
        });
        for (const QFileInfo &info : files) out.append(info.absoluteFilePath());
        QFileInfoList dirs = dir.entryInfoList(QDir::Dirs | QDir::NoDotAndDotDot);
        std::sort(dirs.begin(), dirs.end(), [&](const QFileInfo &a, const QFileInfo &b) {
            return collator.compare(a.fileName(), b.fileName()) < 0;
        });
        for (const QFileInfo &info : dirs)
            scanFolder(info.absoluteFilePath(), rootPath, depth + 1, out);
    }

    void openFolder(const QString &path) {
        fileList_->clear();
        statusBar()->showMessage(tr("Scanning %1…").arg(path));
        auto *watcher = new QFutureWatcher<QStringList>(this);
        connect(watcher, &QFutureWatcher<QStringList>::finished, this, [this, watcher, path] {
            watcher->deleteLater();
            const QStringList files = watcher->result();
            const QDir root(path);
            for (const QString &file : files) {
                const QString relative = root.relativeFilePath(file);
                auto *item = new QListWidgetItem(relative);
                item->setData(Qt::UserRole, file);
                fileList_->addItem(item);
            }
            statusBar()->showMessage(tr("%1 frames").arg(files.size()));
            loadThumbnailsAsync();
            if (fileList_->count() > 0) fileList_->setCurrentRow(0);
        });
        watcher->setFuture(QtConcurrent::run([path] {
            QStringList out;
            scanFolder(path, path, 0, out);
            return out;
        }));
    }

    void selectFile(const QString &path) {
        flushSidecar();  // pending edits belong to the frame being left
        setTool(Tool::None, false);
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
            initHistory(path);
            statusBar()->showMessage(QFileInfo(path).fileName());
            requestRender();
        });
        watcher->setFuture(QtConcurrent::run([path] {
            return si_open(path.toUtf8().constData());
        }));
    }

    // The Mac HQMode.resolve, reduced: tools always render the proxy (their
    // overlays are proxy-space), Off pins proxy, On pins full, Auto serves
    // the medium tier once the canvas is magnified past 2× (a property of
    // the gesture, matching hqAutoZoomThreshold).
    int desiredTier() const {
        if (tool_ != Tool::None) return 0;
        if (hqMode_ == 0) return 0;
        if (hqMode_ == 2) return 2;
        return canvasZoom_ >= 2.0 ? 1 : 0;
    }

    // Latest-wins render queue: at most one in flight, newest request replaces
    // any queued one (the same coalescing discipline as the Mac app).
    // Frames travel zero-copy: the worker writes into the BACK persistent
    // buffer via si_render_into (no malloc, no intermediate copies) while
    // the canvas paints the front one; the swap happens only on success,
    // before any queued render dispatches, so the displayed buffer is never
    // written. The QImage handed to the canvas WRAPS the buffer.
    void requestRender() {
        if (!session_) return;
        if (renderInFlight_) {
            renderQueued_ = true;
            return;
        }
        renderInFlight_ = true;
        const int64_t session = session_;
        const int tier = desiredTier();
        lastTier_ = tier;
        const QByteArray json = QJsonDocument(renderSettings()).toJson(QJsonDocument::Compact);
        const int back = backBuffer_;
        if (!frameBuffers_[back]) frameBuffers_[back] = std::make_shared<QByteArray>();
        auto buffer = frameBuffers_[back];

        auto *watcher = new QFutureWatcher<RenderOutcome>(this);
        connect(watcher, &QFutureWatcher<RenderOutcome>::finished, this, [this, watcher, back] {
            watcher->deleteLater();
            renderInFlight_ = false;
            const RenderOutcome outcome = watcher->result();
            if (!outcome.image.isNull()) {
                backBuffer_ = 1 - back;  // `back` is now on screen; write the other next
                canvas_->setImage(outcome.image);
                histogram_->setBins(outcome.histogram);
                updateInfoRow();
                const char *tierName[] = {"proxy", "HQ", "full"};
                statusBar()->showMessage(
                    tr("render %1 ms · %2 %3×%4 — %5")
                        .arg(outcome.milliseconds).arg(tierName[lastTier_])
                        .arg(outcome.image.width()).arg(outcome.image.height())
                        .arg(deviceName()));
            } else {
                statusBar()->showMessage(tr("Render failed: %1").arg(takeString(si_last_error())));
            }
            if (renderQueued_) {
                renderQueued_ = false;
                requestRender();
            }
        });
        watcher->setFuture(QtConcurrent::run([session, json, tier, buffer] {
            QElapsedTimer timer;
            timer.start();
            RenderOutcome outcome;
            outcome.histogram.resize(1024);
            int32_t w = 0, h = 0;
            int32_t r = si_render_into(session, json.constData(), /*srgb_display=*/1, tier,
                                       reinterpret_cast<uint8_t *>(buffer->data()),
                                       buffer->size(), &w, &h, outcome.histogram.data());
            if (r == -1) {  // buffer too small: size it and go again (source cached)
                buffer->resize(w * h * 4);
                r = si_render_into(session, json.constData(), 1, tier,
                                   reinterpret_cast<uint8_t *>(buffer->data()),
                                   buffer->size(), &w, &h, outcome.histogram.data());
            }
            outcome.milliseconds = timer.elapsed();
            if (r == 1) {
                outcome.image = QImage(reinterpret_cast<uchar *>(buffer->data()), w, h, w * 4,
                                       QImage::Format_RGBA8888);
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

    // The JSON a render should use right now: tools substitute geometry so
    // the canvas shows the space they operate in.
    QJsonObject renderSettings() const {
        QJsonObject s = showingBaseline_ ? QJsonObject() : settings_;
        if (tool_ == Tool::Crop) {
            s.remove("cropRect");
        } else if (tool_ == Tool::Analysis) {
            s.remove("cropRect");
            s.remove("fineRotation");
        }
        return s;
    }

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
        commitHistory(tr("Reset all"));
    }

    void refreshAllControls() {
        for (const auto &refresh : refreshers_) refresh();
        refreshDependentStates();
    }

    void refreshDependentStates() {
        if (separationDampingRow_)
            separationDampingRow_->setEnabled(settingValue("printSaturation") != 1.0);
    }

    // ── History (per file, session-scoped; entries hold full settings) ────

    struct HistoryEntry {
        QString label;
        QJsonObject settings;
    };
    struct FileHistory {
        QVector<HistoryEntry> entries;
        int index = -1;
    };

    FileHistory &history() { return histories_[currentPath_]; }

    void initHistory(const QString &path) {
        if (!histories_.contains(path)) {
            histories_[path].entries = {{tr("Open"), settings_}};
            histories_[path].index = 0;
        }
        updateHistoryList();
    }

    void commitHistory(const QString &label) {
        if (currentPath_.isEmpty() || restoring_) return;
        FileHistory &h = history();
        if (h.index >= 0 && h.entries[h.index].settings == settings_) return;
        h.entries.resize(h.index + 1);  // new edits truncate the redo tail
        h.entries.append({label, settings_});
        if (h.entries.size() > 100) h.entries.removeFirst();
        h.index = h.entries.size() - 1;
        updateHistoryList();
    }

    void jumpHistory(int index) {
        FileHistory &h = history();
        if (index < 0 || index >= h.entries.size() || index == h.index) return;
        h.index = index;
        restoring_ = true;
        settings_ = h.entries[index].settings;
        refreshAllControls();
        restoring_ = false;
        sidecarDirty_ = true;
        saveTimer_->start();
        requestRender();
        updateHistoryList();
    }

    void updateHistoryList() {
        if (!historyList_) return;
        const FileHistory &h = histories_.value(currentPath_);
        historyList_->blockSignals(true);
        historyList_->clear();
        for (const HistoryEntry &e : h.entries) historyList_->addItem(e.label);
        if (h.index >= 0) historyList_->setCurrentRow(h.index);
        historyList_->blockSignals(false);
        historyList_->scrollToBottom();
    }

    // ── Tools ─────────────────────────────────────────────────────────────

    void setTool(Tool tool, bool commitOnExit) {
        if (tool_ == tool) return;
        // Leaving crop without Apply cancels: restore entry-time geometry.
        if (tool_ == Tool::Crop && !commitOnExit) {
            restoring_ = true;
            if (toolSnapshot_.contains("cropRect"))
                settings_.insert("cropRect", toolSnapshot_.value("cropRect"));
            else
                settings_.remove("cropRect");
            settings_.insert("fineRotation", toolSnapshot_.value("fineRotation"));
            restoring_ = false;
        }
        tool_ = tool;
        cropBar_->setVisible(tool == Tool::Crop);
        cropAction_->setChecked(tool == Tool::Crop);
        analysisAction_->setChecked(tool == Tool::Analysis);
        canvas_->setMode(tool == Tool::Crop ? ImageView::Mode::Crop
                         : tool == Tool::Analysis ? ImageView::Mode::AnalysisDraw
                                                  : ImageView::Mode::None);
        if (tool == Tool::Crop) {
            toolSnapshot_ = settings_;
            canvas_->setBox(rectFromJson(settings_.value("cropRect")));
            refreshStraighten();
        } else if (tool == Tool::Analysis) {
            canvas_->setBox(rectFromJson(settings_.value("analysisRect")));
        }
        requestRender();
    }

    void boxCommitted() {
        if (tool_ == Tool::Analysis) {
            const QRectF box = canvas_->box();
            if (box.isEmpty()) {
                settings_.remove("analysisRect");
            } else {
                settings_.insert("analysisRect", rectToJson(box));
                settings_.insert("analysisRectFineRotation", 0.0);
            }
            markDirtyAndRender();  // metering scope changed: show it live
            commitHistory(tr("Analysis region"));
        }
        // Crop mode: the box lives in the canvas until Apply.
    }

    void applyCrop() {
        const QRectF box = canvas_->box();
        const bool nearFull = box.isEmpty() ||
            (box.left() < 0.01 && box.top() < 0.01 && box.right() > 0.99 && box.bottom() > 0.99);
        if (nearFull)
            settings_.remove("cropRect");
        else
            settings_.insert("cropRect", rectToJson(box));
        tool_ = Tool::None;  // skip setTool's cancel path
        cropBar_->setVisible(false);
        cropAction_->setChecked(false);
        canvas_->setMode(ImageView::Mode::None);
        markDirtyAndRender();
        commitHistory(tr("Crop & straighten"));
    }

    void refreshStraighten() {
        QSignalBlocker block(straightenSlider_);
        const double v = settingValue("fineRotation");
        straightenSlider_->setValue(int(std::round(v * 10)));
        straightenValue_->setText(QString::number(v, 'f', 1) + "°");
    }

    // ── UI builders ───────────────────────────────────────────────────────

    void makeToolBar() {
        auto *bar = addToolBar(tr("Tools"));
        bar->setMovable(false);

        auto act = [&](const QString &text, const QKeySequence &shortcut, auto slot) {
            auto *a = bar->addAction(text);
            a->setShortcut(shortcut);
            connect(a, &QAction::triggered, this, slot);
            return a;
        };
        cropAction_ = act(tr("Crop"), QKeySequence("Ctrl+K"), [this] {
            setTool(tool_ == Tool::Crop ? Tool::None : Tool::Crop, false);
        });
        cropAction_->setCheckable(true);
        analysisAction_ = act(tr("Analysis Region"), QKeySequence("Ctrl+Shift+K"), [this] {
            setTool(tool_ == Tool::Analysis ? Tool::None : Tool::Analysis, false);
        });
        analysisAction_->setCheckable(true);
        bar->addSeparator();
        act(tr("Undo"), QKeySequence::Undo, [this] { jumpHistory(history().index - 1); });
        act(tr("Redo"), QKeySequence::Redo, [this] { jumpHistory(history().index + 1); });
        bar->addSeparator();
        exportAction_ = act(tr("Export…"), QKeySequence("Ctrl+E"),
                            [this] { showExportDialog(); });

        // Window-level shortcuts for controls that live on the bottom canvas
        // bar (the Mac's control bar), not in this toolbar.
        auto shortcut = [&](const QKeySequence &seq, auto slot) {
            auto *a = new QAction(this);
            a->setShortcut(seq);
            connect(a, &QAction::triggered, this, slot);
            addAction(a);
        };
        shortcut(QKeySequence("Ctrl+["), [this] { rotate(-90); });
        shortcut(QKeySequence("Ctrl+]"), [this] { rotate(90); });
        shortcut(QKeySequence("Ctrl+Shift+H"), [this] { flip(); });
        shortcut(QKeySequence("Ctrl+Shift+P"), [this] { cycleHQ(); });
    }

    void flip() {
        editSetting("flipHorizontal", !settingBool("flipHorizontal"));
        commitHistory(tr("Flip"));
    }

    void cycleHQ() {
        hqMode_ = (hqMode_ + 1) % 3;
        updateHQBadge();
        if (desiredTier() != lastTier_) requestRender();
    }

    void updateHQBadge() {
        if (!hqBadge_) return;
        const QColor accent = palette().color(QPalette::Highlight);
        QString style;
        switch (hqMode_) {
            case 0:  // off: faint neutral chip
                style = QString("QToolButton { font-weight: 600; padding: 2px 7px;"
                                " border: 1px solid rgba(128,128,128,102);"
                                " border-radius: 4px; background: rgba(128,128,128,15); }");
                break;
            case 1:  // auto: accent tint
                style = QString("QToolButton { font-weight: 600; padding: 2px 7px;"
                                " border: 1px solid rgba(%1,%2,%3,140); border-radius: 4px;"
                                " background: rgba(%1,%2,%3,46); }")
                            .arg(accent.red()).arg(accent.green()).arg(accent.blue());
                break;
            default:  // on: filled accent, light label
                style = QString("QToolButton { font-weight: 600; padding: 2px 7px;"
                                " border: 1px solid rgb(%1,%2,%3); border-radius: 4px;"
                                " background: rgba(%1,%2,%3,217); color: white; }")
                            .arg(accent.red()).arg(accent.green()).arg(accent.blue());
        }
        hqBadge_->setStyleSheet(style);
        const char *tips[] = {"HQ preview off — proxy at any zoom",
                              "HQ preview auto — sharper source past 2× zoom",
                              "HQ preview on — full resolution always"};
        hqBadge_->setToolTip(tr(tips[hqMode_]) + tr(" (Ctrl+Shift+P)"));
    }

    // The Mac's canvas control bar, under the image: rotate/flip, the HQ
    // badge, and the canvas color swatches. (Zones + the densitometer
    // read-out join when those features are ported.)
    QWidget *makeCanvasBar() {
        auto *bar = new QWidget;
        auto *layout = new QHBoxLayout(bar);
        layout->setContentsMargins(12, 4, 12, 4);
        layout->setSpacing(10);

        auto tool = [&](const QString &text, const QString &tip, auto slot) {
            auto *b = new QToolButton;
            b->setText(text);
            b->setToolTip(tip);
            b->setAutoRaise(true);
            connect(b, &QToolButton::clicked, this, slot);
            layout->addWidget(b);
            return b;
        };
        tool(tr("⟲"), tr("Rotate counterclockwise (Ctrl+[)"), [this] { rotate(-90); });
        tool(tr("⟳"), tr("Rotate clockwise (Ctrl+])"), [this] { rotate(90); });
        tool(tr("⇋"), tr("Flip horizontally (Ctrl+Shift+H)"), [this] { flip(); });
        hqBadge_ = tool(tr("HQ"), QString(), [this] { cycleHQ(); });
        updateHQBadge();

        layout->addStretch();

        layout->addWidget(new QLabel(tr("Canvas")));
        QSettings sticky("SwiftInvert", "qt");
        const int savedCanvas = sticky.value("canvasColor", 1).toInt();
        static const QColor swatches[3] = {
            QColor(128, 128, 128), QColor(31, 31, 31), QColor(0, 0, 0)};
        static const char *swatchNames[3] = {"Gray", "Very dark gray", "Black"};
        auto *group = new QButtonGroup(bar);
        group->setExclusive(true);
        for (int i = 0; i < 3; ++i) {
            auto *b = new QToolButton;
            b->setCheckable(true);
            b->setChecked(i == savedCanvas);
            b->setFixedSize(20, 20);
            b->setToolTip(tr(swatchNames[i]));
            b->setStyleSheet(QString(
                "QToolButton { background: %1; border: 1px solid rgba(128,128,128,120);"
                " border-radius: 10px; }"
                "QToolButton:checked { border: 2px solid palette(highlight); }")
                .arg(swatches[i].name()));
            group->addButton(b, i);
            layout->addWidget(b);
        }
        connect(group, &QButtonGroup::idClicked, this, [this](int i) {
            static const QColor colors[3] = {
                QColor(128, 128, 128), QColor(31, 31, 31), QColor(0, 0, 0)};
            canvas_->setCanvasColor(colors[i]);
            QSettings("SwiftInvert", "qt").setValue("canvasColor", i);
        });
        canvas_->setCanvasColor(swatches[savedCanvas]);
        return bar;
    }

    void rotate(int delta) {
        const int rotation = (int(settingValue("rotation")) + delta + 360) % 360;
        editSetting("rotation", rotation);
        commitHistory(delta > 0 ? tr("Rotate right") : tr("Rotate left"));
    }

    QWidget *makeCropBar() {
        cropBar_ = new QWidget;
        auto *layout = new QHBoxLayout(cropBar_);
        layout->setContentsMargins(8, 4, 8, 4);
        layout->addWidget(new QLabel(tr("Straighten")));
        straightenSlider_ = new QSlider(Qt::Horizontal);
        straightenSlider_->setRange(-450, 450);  // ±45° in 0.1° steps
        straightenSlider_->setMaximumWidth(320);
        straightenValue_ = new QLabel("0.0°");
        straightenValue_->setMinimumWidth(46);
        straightenSlider_->installEventFilter(this);
        connect(straightenSlider_, &QSlider::valueChanged, this, [this](int t) {
            const double v = t / 10.0;
            straightenValue_->setText(QString::number(v, 'f', 1) + "°");
            if (!refreshing_) {
                settings_.insert("fineRotation", v);
                requestRender();  // committed with Apply, not per tick
            }
        });
        layout->addWidget(straightenSlider_);
        layout->addWidget(straightenValue_);
        layout->addSpacing(12);
        auto *apply = new QPushButton(tr("Apply"));
        connect(apply, &QPushButton::clicked, this, &MainWindow::applyCrop);
        auto *clear = new QPushButton(tr("Clear"));
        connect(clear, &QPushButton::clicked, this, [this] {
            canvas_->setBox(QRectF());
            straightenSlider_->setValue(0);
        });
        auto *cancel = new QPushButton(tr("Cancel"));
        connect(cancel, &QPushButton::clicked, this, [this] { setTool(Tool::None, false); });
        layout->addWidget(apply);
        layout->addWidget(clear);
        layout->addWidget(cancel);
        layout->addStretch();
        cropBar_->setVisible(false);
        return cropBar_;
    }

    // One slider row: caption + reset-⨯ (visible when off-default) + value
    // label; double-click resets; history commits on release (drags) or
    // immediately (programmatic sets like the reset buttons).
    QWidget *sliderRow(const QString &label, double minimum, double maximum, double step,
                      double defaultValue, int decimals, const QString &suffix,
                      std::function<double()> get, std::function<void(double)> set,
                      const QStringList &gradient = {}, QSlider **outSlider = nullptr) {
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
        if (!gradient.isEmpty()) slider->setStyleSheet(gradientStyle(gradient));
        if (outSlider) *outSlider = slider;

        auto toTicks = [minimum, step](double v) { return int((v - minimum) / step + 0.5); };
        auto fromTicks = [minimum, step](int t) { return minimum + t * step; };
        auto updateLabels = [value, reset, defaultValue, decimals, suffix](double v) {
            value->setText(QString::number(v, 'f', decimals) + suffix);
            const bool changed = std::abs(v - defaultValue) > 1e-9;
            reset->setVisible(changed);
            value->setStyleSheet(changed ? "font-size: 11px;"
                                         : "font-size: 11px; color: gray;");
        };

        connect(slider, &QSlider::valueChanged, this,
                [this, set, fromTicks, updateLabels, label, slider](int t) {
                    const double v = fromTicks(t);
                    updateLabels(v);
                    if (refreshing_) return;
                    set(v);
                    if (!slider->isSliderDown()) commitHistory(label);
                });
        connect(slider, &QSlider::sliderReleased, this, [this, label] { commitHistory(label); });
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
                           const QString &suffix = QString(), const QStringList &gradient = {}) {
        return sliderRow(
            label, minimum, maximum, step, defaultValue, decimals, suffix,
            [this, key] { return settingValue(key); },
            [this, key](double v) { editSetting(key, v); }, gradient);
    }

    QCheckBox *settingToggle(const QString &label, const QString &key) {
        auto *box = new QCheckBox(label);
        box->setChecked(settingBool(key));
        connect(box, &QCheckBox::toggled, this, [this, key, label](bool on) {
            if (refreshing_) return;
            editSetting(key, on);
            commitHistory(label);
        });
        refreshers_.push_back([this, box, key] {
            refreshing_ = true;
            QSignalBlocker block(box);
            box->setChecked(settingBool(key));
            refreshing_ = false;
        });
        return box;
    }

    QWidget *bandPicker(const QStringList &names, int initial, std::function<void(int)> onChange) {
        auto *row = new QWidget;
        auto *layout = new QHBoxLayout(row);
        layout->setContentsMargins(0, 0, 0, 0);
        layout->setSpacing(0);
        auto *group = new QButtonGroup(row);
        group->setExclusive(true);
        for (int i = 0; i < names.size(); ++i) {
            auto *b = new QPushButton(names[i]);
            b->setCheckable(true);
            b->setChecked(i == initial);
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

        auto *viewOriginal = new QPushButton(tr("View Original (hold)"));
        connect(viewOriginal, &QPushButton::pressed, this, [this] {
            showingBaseline_ = true;
            requestRender();
        });
        connect(viewOriginal, &QPushButton::released, this, [this] {
            showingBaseline_ = false;
            requestRender();
        });
        auto *clearAnalysis = new QPushButton(tr("Clear Analysis Region"));
        connect(clearAnalysis, &QPushButton::clicked, this, [this] {
            if (!settings_.contains("analysisRect")) return;
            settings_.remove("analysisRect");
            if (tool_ == Tool::Analysis) canvas_->setBox(QRectF());
            markDirtyAndRender();
            commitHistory(tr("Clear analysis region"));
        });
        layout->addWidget(section(tr("Pre-process"), {viewOriginal, clearAnalysis}));

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

        {
            static const char *hueKeys[4] = {"redHue", "yellowHue", "greenHue", "blueHue"};
            static const char *satKeys[4] = {"redSaturation", "yellowSaturation",
                                             "greenSaturation", "blueSaturation"};
            // Track colors per band: [− destination, band color, + destination]
            // (+ rotates ccw in Lab: red→orange, yellow→green, green→teal,
            // blue→purple) — ColorMixerSection.hueColors verbatim.
            static const QStringList hueGradients[4] = {
                {"#D93380", "#D93333", "#E68C26"},
                {"#E68C26", "#E0CC33", "#99D140"},
                {"#99D140", "#40B84D", "#26B899"},
                {"#26ADCC", "#4073E0", "#8C4DD9"},
            };
            auto satGradient = [](int band) {
                return QStringList{"#666666", hueGradients[band][1]};
            };
            QSlider *hueSlider = nullptr;
            QSlider *satSlider = nullptr;
            auto *hueRow = sliderRow(
                tr("Hue"), -1.5, 1.5, 0.01, 0, 2, QString(),
                [this] { return settingValue(hueKeys[mixerBand_]); },
                [this](double v) { editSetting(hueKeys[mixerBand_], v); },
                hueGradients[0], &hueSlider);
            auto *satRow = sliderRow(
                tr("Saturation"), 0, 2, 0.01, 1.0, 2, QString(),
                [this] { return settingValue(satKeys[mixerBand_]); },
                [this](double v) { editSetting(satKeys[mixerBand_], v); },
                satGradient(0), &satSlider);
            auto *picker = bandPicker(
                {tr("R"), tr("Y"), tr("G"), tr("B")}, 0,
                [this, hueSlider, satSlider, satGradient](int band) {
                    mixerBand_ = band;
                    hueSlider->setStyleSheet(gradientStyle(hueGradients[band]));
                    satSlider->setStyleSheet(gradientStyle(satGradient(band)));
                    refreshAllControls();
                });
            layout->addWidget(section(tr("Color Mixer"), {picker, hueRow, satRow}));
        }

        {
            static const char *bandKeys[3] = {"colorShadows", "colorMids", "colorHighs"};
            auto *picker =
                bandPicker({tr("Shadows"), tr("Mids"), tr("Highs")}, 1, [this](int band) {
                    gradingBand_ = band;
                    refreshAllControls();
                });
            auto channelSlider = [this](const QString &label, int component,
                                        const QStringList &gradient) {
                return sliderRow(
                    label, -1, 1, 0.01, 0, 2, QString(),
                    [this, component] { return gradingValue(bandKeys[gradingBand_], component); },
                    [this, component](double v) { editGrading(bandKeys[gradingBand_], component, v); },
                    gradient);
            };
            layout->addWidget(section(
                tr("Color Grading"),
                {
                    settingSlider(tr("Temp"), "temp", -1, 1, 0.01, 0, 2, QString(),
                                  {"#4059D9", "#666666", "#BFB326"}),
                    settingSlider(tr("Tint"), "tint", -1, 1, 0.01, 0, 2, QString(),
                                  {"#33BF40", "#666666", "#B333BF"}),
                    picker,
                    channelSlider(tr("R ↔ C"), 0, {"#D93333", "#666666", "#1ABFCC"}),
                    channelSlider(tr("G ↔ M"), 1, {"#33BF40", "#666666", "#B333BF"}),
                    channelSlider(tr("B ↔ Y"), 2, {"#4059D9", "#666666", "#CCBF33"}),
                }));
        }

        layout->addWidget(section(
            tr("Tone"),
            {
                settingSlider(tr("Toe"), "toe", -1, 1, 0.01, 0),
                settingSlider(tr("Shoulder"), "shoulder", -1, 1, 0.01, 0),
                settingToggle(tr("True black"), "trueBlack"),
                settingSlider(tr("White point"), "whitePointOffset", -0.3, 0.3, 0.001, 0, 3),
                settingSlider(tr("Black point"), "blackPointOffset", -0.3, 0.3, 0.001, 0, 3),
            }));

        // History: per-file entries, click to jump (undo/redo walk the same
        // list). Kept at the bottom like the Mac's HistoryPanel.
        historyList_ = new QListWidget;
        historyList_->setFixedHeight(110);
        historyList_->setStyleSheet("font-size: 11px;");
        connect(historyList_, &QListWidget::itemClicked, this,
                [this](QListWidgetItem *item) { jumpHistory(historyList_->row(item)); });
        layout->addWidget(section(tr("History"), {historyList_}));

        layout->addStretch();
        refreshDependentStates();

        auto *scroll = new QScrollArea;
        scroll->setWidget(panel);
        scroll->setWidgetResizable(true);
        scroll->setMinimumWidth(340);
        scroll->setMaximumWidth(400);
        scroll->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
        controlsScroll_ = scroll;
        return scroll;
    }

    // ── Export ────────────────────────────────────────────────────────────

    // Options dialog mirroring the Mac's ExportSheet: format, colorspace,
    // resize, JPEG quality, destination (next-to-source or a folder), with
    // sticky last-used values. Exports every selected frame (or the current
    // one) using each frame's own sidecar; the current frame uses the live
    // settings (flushed to its sidecar first).
    void showExportDialog() {
        QStringList paths;
        for (QListWidgetItem *item : fileList_->selectedItems())
            paths.append(item->data(Qt::UserRole).toString());
        if (paths.isEmpty() && !currentPath_.isEmpty()) paths.append(currentPath_);
        if (paths.isEmpty()) return;

        QSettings sticky("SwiftInvert", "qt");
        QDialog dialog(this);
        dialog.setWindowTitle(tr("Export %n frame(s)", nullptr, paths.size()));
        auto *form = new QFormLayout(&dialog);

        auto *format = new QComboBox;
        format->addItems({tr("JPEG"), tr("TIFF (16-bit)")});
        format->setCurrentIndex(sticky.value("export/format", 0).toInt());
        form->addRow(tr("Format"), format);

        auto *colorspace = new QComboBox;
        colorspace->addItems({tr("sRGB"), tr("Adobe RGB (wide gamut)")});
        colorspace->setCurrentIndex(sticky.value("export/colorspace", 0).toInt());
        form->addRow(tr("Color space"), colorspace);

        auto *quality = new QSpinBox;
        quality->setRange(50, 100);
        quality->setValue(sticky.value("export/quality", 92).toInt());
        form->addRow(tr("JPEG quality"), quality);
        auto syncQuality = [format, quality] { quality->setEnabled(format->currentIndex() == 0); };
        connect(format, &QComboBox::currentIndexChanged, &dialog, syncQuality);
        syncQuality();

        auto *resize = new QCheckBox(tr("Resize long edge to"));
        resize->setChecked(sticky.value("export/resize", true).toBool());
        auto *edge = new QSpinBox;
        edge->setRange(16, 20000);
        edge->setValue(sticky.value("export/maxLongEdge", 3000).toInt());
        auto *resizeRow = new QWidget;
        auto *resizeLayout = new QHBoxLayout(resizeRow);
        resizeLayout->setContentsMargins(0, 0, 0, 0);
        resizeLayout->addWidget(resize);
        resizeLayout->addWidget(edge);
        connect(resize, &QCheckBox::toggled, edge, &QWidget::setEnabled);
        edge->setEnabled(resize->isChecked());
        form->addRow(resizeRow);

        auto *nextToSource = new QRadioButton(tr("Next to source"));
        auto *toFolder = new QRadioButton(tr("Folder:"));
        auto *folderEdit = new QLineEdit(sticky.value("export/folder").toString());
        auto *browse = new QPushButton(tr("…"));
        browse->setMaximumWidth(32);
        connect(browse, &QPushButton::clicked, &dialog, [&dialog, folderEdit, toFolder] {
            const QString dir = QFileDialog::getExistingDirectory(&dialog, tr("Export Folder"));
            if (!dir.isEmpty()) {
                folderEdit->setText(dir);
                toFolder->setChecked(true);
            }
        });
        (sticky.value("export/toFolder", false).toBool() ? toFolder : nextToSource)->setChecked(true);
        auto *destRow = new QWidget;
        auto *destLayout = new QHBoxLayout(destRow);
        destLayout->setContentsMargins(0, 0, 0, 0);
        destLayout->addWidget(nextToSource);
        destLayout->addWidget(toFolder);
        destLayout->addWidget(folderEdit, 1);
        destLayout->addWidget(browse);
        form->addRow(destRow);

        auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
        buttons->button(QDialogButtonBox::Ok)->setText(tr("Export"));
        connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
        connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
        form->addRow(buttons);

        if (dialog.exec() != QDialog::Accepted) return;

        sticky.setValue("export/format", format->currentIndex());
        sticky.setValue("export/colorspace", colorspace->currentIndex());
        sticky.setValue("export/quality", quality->value());
        sticky.setValue("export/resize", resize->isChecked());
        sticky.setValue("export/maxLongEdge", edge->value());
        sticky.setValue("export/toFolder", toFolder->isChecked());
        sticky.setValue("export/folder", folderEdit->text());

        QJsonObject options;
        options.insert("colorspace", colorspace->currentIndex() == 0 ? "srgb" : "adobe");
        if (resize->isChecked()) options.insert("maxLongEdge", edge->value());
        runExport(paths, format->currentIndex() == 1, options, quality->value(),
                  toFolder->isChecked() ? folderEdit->text() : QString());
    }

    void runExport(const QStringList &paths, bool tiff, const QJsonObject &options,
                   int quality, const QString &folder) {
        flushSidecar();  // the current frame's live edits become its sidecar
        exportAction_->setEnabled(false);
        const QByteArray optionsJson = QJsonDocument(options).toJson(QJsonDocument::Compact);
        auto *self = this;
        (void)QtConcurrent::run([self, paths, tiff, optionsJson, quality, folder] {
            int done = 0, failed = 0;
            for (const QString &path : paths) {
                QMetaObject::invokeMethod(self, [self, done, total = paths.size(), path] {
                    self->statusBar()->showMessage(
                        tr("Exporting %1/%2 — %3…").arg(done + 1).arg(total)
                            .arg(QFileInfo(path).fileName()));
                }, Qt::QueuedConnection);

                const QFileInfo info(path);
                const QString dir = folder.isEmpty() ? info.absolutePath() : folder;
                const QString dest = dir + "/" + info.completeBaseName() + (tiff ? ".tiff" : ".jpg");
                char *sidecar = si_sidecar_load(path.toUtf8().constData());
                const QByteArray settings = sidecar ? QByteArray(sidecar) : QByteArray("{}");
                if (sidecar) si_free(sidecar);

                bool ok = false;
                if (tiff) {
                    ok = si_export_tiff(path.toUtf8().constData(), dest.toUtf8().constData(),
                                        settings.constData(), optionsJson.constData());
                } else {
                    int32_t w = 0, h = 0;
                    uint8_t *rgba = si_export_render(path.toUtf8().constData(), settings.constData(),
                                                     optionsJson.constData(), &w, &h);
                    if (rgba) {
                        const QImage img(rgba, w, h, w * 4, QImage::Format_RGBA8888);
                        ok = img.save(dest, "JPEG", quality);
                        si_free(rgba);
                    }
                }
                ok ? ++done : ++failed;
            }
            QMetaObject::invokeMethod(self, [self, done, failed] {
                self->exportAction_->setEnabled(true);
                self->statusBar()->showMessage(
                    failed ? tr("Exported %1, failed %2: %3").arg(done).arg(failed)
                                 .arg(takeString(si_last_error()))
                           : tr("Exported %1 frame(s)").arg(done));
            }, Qt::QueuedConnection);
        });
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
        fileList_->setSelectionMode(QAbstractItemView::ExtendedSelection);
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
        auto alive = alive_;
        (void)QtConcurrent::run([self, alive, paths, generation] {
            for (int i = 0; i < paths.size(); ++i) {
                if (!alive->load()) return;  // window gone: stop touching it
                int32_t length = 0;
                uint8_t *jpeg = si_thumbnail(paths[i].toUtf8().constData(), &length);
                if (!jpeg) continue;
                QImage image = QImage::fromData(jpeg, length);
                si_free(jpeg);
                if (image.isNull()) continue;
                // QImage is thread-safe to build here; QPixmap/QIcon are
                // GUI-thread-only, so the conversion happens in the lambda.
                image = image.scaled(148, 112, Qt::KeepAspectRatio, Qt::SmoothTransformation);
                if (!alive->load()) return;
                QMetaObject::invokeMethod(self, [self, i, image, generation] {
                    if (generation != self->thumbGeneration_) return;
                    if (i < self->fileList_->count())
                        self->fileList_->item(i)->setIcon(QIcon(QPixmap::fromImage(image)));
                }, Qt::QueuedConnection);
            }
        });
    }

    void updateInfoRow() {
        if (!session_) return;
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
        // The wheel must never change a slider (mouse drag only) — over a
        // slider it scrolls the controls panel instead, like any other spot.
        if (event->type() == QEvent::Wheel) {
            if (auto *slider = qobject_cast<QSlider *>(object)) {
                auto *wheel = static_cast<QWheelEvent *>(event);
                if (controlsScroll_ && controlsScroll_->isAncestorOf(slider)) {
                    QWheelEvent forwarded(
                        controlsScroll_->viewport()->mapFromGlobal(wheel->globalPosition()),
                        wheel->globalPosition(), wheel->pixelDelta(), wheel->angleDelta(),
                        wheel->buttons(), wheel->modifiers(), wheel->phase(),
                        wheel->inverted());
                    QApplication::sendEvent(controlsScroll_->viewport(), &forwarded);
                }
                return true;
            }
        }
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
    QWidget *cropBar_ = nullptr;
    QSlider *straightenSlider_ = nullptr;
    QLabel *straightenValue_ = nullptr;
    QAction *cropAction_ = nullptr;
    QAction *analysisAction_ = nullptr;
    QAction *exportAction_ = nullptr;
    QToolButton *hqBadge_ = nullptr;
    QScrollArea *controlsScroll_ = nullptr;
    QListWidget *historyList_ = nullptr;
    QJsonObject defaults_;
    QJsonObject settings_;
    QJsonObject toolSnapshot_;
    std::vector<std::function<void()>> refreshers_;
    QHash<QSlider *, int> sliderDefaults_;
    QHash<QString, FileHistory> histories_;
    QTimer *saveTimer_ = nullptr;
    QString currentPath_;
    QString cachedDevice_;
    int64_t session_ = 0;
    Tool tool_ = Tool::None;
    int hqMode_ = 1;  // 0 = Off, 1 = Auto (medium at ≥2× zoom), 2 = On (full)
    double canvasZoom_ = 1.0;
    int lastTier_ = 0;
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

private:
    std::shared_ptr<std::atomic_bool> alive_ = std::make_shared<std::atomic_bool>(true);
    std::shared_ptr<QByteArray> frameBuffers_[2];
    int backBuffer_ = 0;
};

// Headless proof: open a file (or a folder's first RAW), render synchronously
// through the same code paths, screenshot the whole window — then again in
// crop mode with a box and a straighten angle (<base>_crop.png). Run with
// QT_QPA_PLATFORM=offscreen for CI-style checks.
bool MainWindow::selfTest(const QString &target, const QString &screenshotPath) {
    QFileInfo info(target);
    QString file = target;
    openFolder(info.isDir() ? target : info.absolutePath());
    // The folder scan is async now — pump until the filmstrip populates.
    QDeadlineTimer scanDeadline(8000);
    while (fileList_->count() == 0 && !scanDeadline.hasExpired())
        QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
    if (info.isDir()) {
        if (fileList_->count() == 0) return false;
        file = fileList_->item(0)->data(Qt::UserRole).toString();
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
    initHistory(file);

    auto renderSync = [this]() -> bool {
        int32_t w = 0, h = 0;
        QVector<quint32> bins(1024);
        const QByteArray json = QJsonDocument(renderSettings()).toJson(QJsonDocument::Compact);
        uint8_t *rgba = si_render(session_, json.constData(), 1, 0, &w, &h, bins.data());
        if (!rgba) {
            fprintf(stderr, "selftest: %s\n", qPrintable(takeString(si_last_error())));
            return false;
        }
        canvas_->setImage(QImage(rgba, w, h, w * 4, QImage::Format_RGBA8888).copy());
        si_free(rgba);
        histogram_->setBins(bins);
        updateInfoRow();
        statusBar()->showMessage(tr("selftest — %1x%2 via %3").arg(w).arg(h).arg(deviceName()));
        return true;
    };
    if (!renderSync()) return false;
    show();
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
    if (!grab().save(screenshotPath)) return false;

    // Second shot: crop mode with a straighten angle and a box.
    setTool(Tool::Crop, false);
    settings_.insert("fineRotation", 3.0);
    refreshStraighten();
    canvas_->setBox(QRectF(0.12, 0.10, 0.72, 0.75));
    if (!renderSync()) return false;
    QCoreApplication::processEvents();
    QString cropShot = screenshotPath;
    cropShot.replace(QStringLiteral(".png"), QStringLiteral("_crop.png"));
    if (!grab().save(cropShot)) return false;
    QString panelShot = screenshotPath;
    panelShot.replace(QStringLiteral(".png"), QStringLiteral("_controls.png"));
    return controlsScroll_->widget()->grab().save(panelShot);
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
