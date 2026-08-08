// SwiftInvert's Linux shell: Qt Widgets over the SwiftInvertCore C bridge.
// Deliberately thin — decode/analysis/render/settings semantics all live in
// the Swift core; this file is folder browsing, a canvas, sliders that edit
// a settings JSON object, and latest-wins async rendering.

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
    qint64 milliseconds = 0;
};

}  // namespace

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

class MainWindow : public QMainWindow {
public:
    MainWindow() {
        defaults_ = QJsonDocument::fromJson(takeString(si_default_settings()).toUtf8()).object();
        settings_ = defaults_;

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
        resize(1440, 900);
        statusBar()->showMessage(tr("Ready"));

        auto *openAction = new QAction(tr("&Open Folder…"), this);
        openAction->setShortcut(QKeySequence::Open);
        connect(openAction, &QAction::triggered, this, &MainWindow::chooseFolder);
        menuBar()->addMenu(tr("&File"))->addAction(openAction);
    }

    ~MainWindow() override {
        if (session_) si_close(session_);
    }

    void openFolder(const QString &path) {
        fileList_->clear();
        const QDir dir(path);
        const QFileInfoList files =
            dir.entryInfoList(kRawPatterns, QDir::Files, QDir::Name);
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
            statusBar()->showMessage(tr("%1").arg(QFileInfo(path).fileName()));
            requestRender();
        });
        watcher->setFuture(QtConcurrent::run([path] {
            return si_open(path.toUtf8().constData());
        }));
    }

    // Latest-wins render queue: at most one in flight, newest request replaces
    // any queued one (the Swift side mirrors the app's coalescing behavior).
    void requestRender() {
        if (!session_) return;
        if (renderInFlight_) {
            renderQueued_ = true;
            return;
        }
        renderInFlight_ = true;
        const int64_t session = session_;
        const QByteArray json = QJsonDocument(settings_).toJson(QJsonDocument::Compact);
        auto *watcher = new QFutureWatcher<RenderOutcome>(this);
        connect(watcher, &QFutureWatcher<RenderOutcome>::finished, this, [this, watcher] {
            watcher->deleteLater();
            renderInFlight_ = false;
            const RenderOutcome outcome = watcher->result();
            if (!outcome.image.isNull()) {
                canvas_->setImage(outcome.image);
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
            int32_t w = 0, h = 0;
            uint8_t *rgba = si_render(session, json.constData(), &w, &h);
            RenderOutcome outcome;
            outcome.milliseconds = timer.elapsed();
            if (rgba) {
                // Deep-copy into a QImage that owns its bytes, then free.
                outcome.image =
                    QImage(rgba, w, h, w * 4, QImage::Format_RGBA8888).copy();
                si_free(rgba);
            }
            return outcome;
        }));
    }

    bool selfTest(const QString &target, const QString &screenshotPath);

private:
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

    void addSlider(QFormLayout *form, const QString &label, const QString &key,
                   double minimum, double maximum, double step) {
        auto *slider = new QSlider(Qt::Horizontal);
        const int ticks = int((maximum - minimum) / step + 0.5);
        slider->setRange(0, ticks);
        const double defaultValue = defaults_.value(key).toDouble();
        auto toTicks = [minimum, step](double v) { return int((v - minimum) / step + 0.5); };
        slider->setValue(toTicks(defaultValue));
        auto *value = new QLabel(QString::number(defaultValue, 'f', 2));
        value->setMinimumWidth(44);
        auto *row = new QWidget;
        auto *rowLayout = new QHBoxLayout(row);
        rowLayout->setContentsMargins(0, 0, 0, 0);
        rowLayout->addWidget(slider, 1);
        rowLayout->addWidget(value);
        form->addRow(label, row);
        sliders_.insert(key, {slider, value, minimum, step});
        connect(slider, &QSlider::valueChanged, this, [this, key, minimum, step, value](int t) {
            const double v = minimum + t * step;
            value->setText(QString::number(v, 'f', 2));
            settings_.insert(key, v);
            requestRender();
        });
    }

    void addToggle(QFormLayout *form, const QString &label, const QString &key) {
        auto *box = new QCheckBox(label);
        box->setChecked(defaults_.value(key).toBool());
        form->addRow(box);
        toggles_.insert(key, box);
        connect(box, &QCheckBox::toggled, this, [this, key](bool on) {
            settings_.insert(key, on);
            requestRender();
        });
    }

    QWidget *makeControlsPanel() {
        auto *panel = new QWidget;
        auto *form = new QFormLayout(panel);
        form->setLabelAlignment(Qt::AlignRight);

        addToggle(form, tr("Auto Exposure"), "autoExposure");
        addToggle(form, tr("Auto Contrast"), "autoNormalizeContrast");
        addSlider(form, tr("Brightness"), "density", 0.2, 2.5, 0.01);
        addSlider(form, tr("Grade"), "grade", 50, 180, 1);
        addSlider(form, tr("Cyan"), "wbCyan", -1, 1, 0.01);
        addSlider(form, tr("Magenta"), "wbMagenta", -1, 1, 0.01);
        addSlider(form, tr("Yellow"), "wbYellow", -1, 1, 0.01);
        addSlider(form, tr("Exposure"), "exposureStops", -2, 2, 0.05);
        addSlider(form, tr("Cast Removal"), "castRemovalStrength", 0, 2, 0.01);
        addSlider(form, tr("Shadows"), "shadows", -1, 1, 0.01);
        addSlider(form, tr("Highlights"), "highlights", -1, 1, 0.01);
        addSlider(form, tr("Vibrance"), "vibrance", 0, 2, 0.01);
        addSlider(form, tr("Saturation"), "saturation", 0, 2, 0.01);
        addToggle(form, tr("True Black"), "trueBlack");

        auto *reset = new QPushButton(tr("Reset All"));
        connect(reset, &QPushButton::clicked, this, &MainWindow::resetAll);
        form->addRow(reset);

        auto *scroll = new QScrollArea;
        scroll->setWidget(panel);
        scroll->setWidgetResizable(true);
        scroll->setMinimumWidth(320);
        scroll->setMaximumWidth(380);
        return scroll;
    }

    void resetAll() {
        settings_ = defaults_;
        for (auto it = sliders_.constBegin(); it != sliders_.constEnd(); ++it) {
            const SliderRow &row = it.value();
            const double v = defaults_.value(it.key()).toDouble();
            QSignalBlocker block(row.slider);
            row.slider->setValue(int((v - row.minimum) / row.step + 0.5));
            row.value->setText(QString::number(v, 'f', 2));
        }
        for (auto it = toggles_.constBegin(); it != toggles_.constEnd(); ++it) {
            QSignalBlocker block(it.value());
            it.value()->setChecked(defaults_.value(it.key()).toBool());
        }
        requestRender();
    }

    struct SliderRow {
        QSlider *slider;
        QLabel *value;
        double minimum;
        double step;
    };

    QListWidget *fileList_ = nullptr;
    ImageView *canvas_ = nullptr;
    QJsonObject defaults_;
    QJsonObject settings_;
    QHash<QString, SliderRow> sliders_;
    QHash<QString, QCheckBox *> toggles_;
    int64_t session_ = 0;
    QString cachedDevice_;
    quint64 openGeneration_ = 0;
    bool renderInFlight_ = false;
    bool renderQueued_ = false;

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
    int32_t w = 0, h = 0;
    const QByteArray json = QJsonDocument(settings_).toJson(QJsonDocument::Compact);
    uint8_t *rgba = si_render(session_, json.constData(), &w, &h);
    if (!rgba) {
        fprintf(stderr, "selftest: %s\n", qPrintable(takeString(si_last_error())));
        return false;
    }
    canvas_->setImage(QImage(rgba, w, h, w * 4, QImage::Format_RGBA8888).copy());
    si_free(rgba);
    statusBar()->showMessage(
        tr("selftest — %1x%2 via %3").arg(w).arg(h).arg(deviceName()));
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
