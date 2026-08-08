#include "levelswindow.h"

#include <QtWidgets>
#include <algorithm>

namespace {
constexpr double kGap = 0.01;            // min gap between neighbours, both axes
constexpr double kGrabTolerance = 0.015; // grab radius around an anchor's line
constexpr double kClamp = 0.02;          // ExposureSettings.levelsClamp
constexpr int kMaxPoints = 8;            // ExposureSettings.levelsMaxPoints
constexpr int kRemovalStrip = 20;        // px under the plot for the ✕ row

// ReferenceCurve.levelsInverseRemap: displayed position → input tone.
double inverseRemap(double y, const QVector<QPointF> &points) {
    double x0 = 0.0, y0 = 0.0;
    for (const QPointF &p : points) {
        if (y <= p.y()) {
            const double dy = p.y() - y0;
            return dy < 1e-6 ? p.x() : x0 + (y - y0) * (p.x() - x0) / dy;
        }
        x0 = p.x();
        y0 = p.y();
    }
    const double dy = 1.0 - y0;
    return dy < 1e-6 ? 1.0 : x0 + (y - y0) * (1.0 - x0) / dy;
}

const QColor kChannelColors[3] = {
    QColor(235, 70, 70), QColor(80, 220, 80), QColor(90, 130, 250)};
}  // namespace

// ── Plot: histogram + anchor lines + drag editing + removal strip ─────────

class LevelsPlot : public QWidget {
public:
    LevelsWindow *host = nullptr;
    std::function<QVector<QPointF>(int)> getAnchors;
    std::function<void(int, const QVector<QPointF> &)> setAnchors;
    std::function<void(const QString &)> commitHistory;
    int channel = 0;

    explicit LevelsPlot(QWidget *parent = nullptr) : QWidget(parent) {
        setMinimumSize(520, 240);
        setMouseTracking(false);
    }

    void setBins(const QVector<quint32> &bins) {
        bins_ = bins;
        update();
    }

protected:
    QRectF plotRect() const {
        return QRectF(6, 6, width() - 12, height() - 12 - kRemovalStrip);
    }

    void paintEvent(QPaintEvent *) override {
        QPainter p(this);
        p.fillRect(rect(), palette().window());
        const QRectF r = plotRect();
        p.setRenderHint(QPainter::Antialiasing);
        p.setPen(Qt::NoPen);
        p.setBrush(QColor(20, 20, 22));
        p.drawRoundedRect(r.adjusted(-4, -4, 4, 4 + kRemovalStrip), 6, 6);

        // Inactive channels dimmed behind, active on top (linear scale,
        // peak-normalized per channel — mirrors the Mac plot).
        if (bins_.size() >= 768) {
            for (int ch = 0; ch < 3; ++ch)
                if (ch != channel) drawChannel(p, r, ch, 0.25);
            drawChannel(p, r, channel, 0.9);
        }

        // Anchor lines at their OUTPUT positions; the dragged one thicker,
        // with its input origin dashed white. ✕ glyphs in the strip below.
        const QVector<QPointF> anchors = getAnchors(channel);
        for (int i = 0; i < anchors.size(); ++i) {
            const double x = r.left() + anchors[i].y() * r.width();
            QColor c = kChannelColors[channel];
            c.setAlphaF(i == dragIndex_ ? 1.0 : 0.75);
            p.setPen(QPen(c, i == dragIndex_ ? 2.0 : 1.2));
            p.drawLine(QPointF(x, r.top()), QPointF(x, r.bottom()));
            if (i == dragIndex_) {
                QPen dashed(QColor(255, 255, 255, 80), 1, Qt::DashLine);
                p.setPen(dashed);
                const double ix = r.left() + anchors[i].x() * r.width();
                p.drawLine(QPointF(ix, r.top()), QPointF(ix, r.bottom()));
            }
            p.setPen(QPen(QColor(190, 190, 190), 1.4));
            const QPointF c1(x, r.bottom() + kRemovalStrip / 2.0 + 2);
            const double s = 3.2;
            p.drawLine(c1 + QPointF(-s, -s), c1 + QPointF(s, s));
            p.drawLine(c1 + QPointF(-s, s), c1 + QPointF(s, -s));
        }
    }

    void mousePressEvent(QMouseEvent *event) override {
        if (event->button() != Qt::LeftButton) return;
        const QRectF r = plotRect();
        const double x =
            std::clamp((event->position().x() - r.left()) / r.width(), 0.0, 1.0);

        // ✕ strip: remove the nearest anchor whose ✕ we pressed.
        if (event->position().y() > r.bottom()) {
            QVector<QPointF> pts = getAnchors(channel);
            for (int i = 0; i < pts.size(); ++i) {
                if (std::abs(pts[i].y() - x) * r.width() <= 8) {
                    pts.remove(i);
                    setAnchors(channel, pts);
                    commitHistory(
                        tr("%1 anchor removed").arg(LevelsWindow::tr(
                            channel == 0 ? "Red" : channel == 1 ? "Green" : "Blue")));
                    update();
                    return;
                }
            }
            return;
        }
        if (event->position().y() < r.top()) return;
        dragIndex_ = grabOrPlantAnchor(x);
        if (dragIndex_ >= 0) moveAnchor(dragIndex_, x);
        update();
    }

    void mouseMoveEvent(QMouseEvent *event) override {
        if (dragIndex_ < 0) return;
        const QRectF r = plotRect();
        const double x =
            std::clamp((event->position().x() - r.left()) / r.width(), 0.0, 1.0);
        moveAnchor(dragIndex_, x);
        update();
    }

    void mouseReleaseEvent(QMouseEvent *event) override {
        if (event->button() != Qt::LeftButton || dragIndex_ < 0) return;
        dragIndex_ = -1;
        commitHistory(tr("%1 levels").arg(
            LevelsWindow::tr(channel == 0 ? "Red" : channel == 1 ? "Green" : "Blue")));
        update();
    }

private:
    // Grab an existing anchor when the press lands near its line, else plant
    // a new one whose input is the tone currently DISPLAYED at the press
    // position (inverse of the remap in effect). At capacity the nearest
    // anchor is grabbed instead. Returns -1 when there's no room to plant.
    int grabOrPlantAnchor(double x) {
        QVector<QPointF> pts = getAnchors(channel);
        int nearest = -1;
        double nearestDist = 1e9;
        for (int i = 0; i < pts.size(); ++i) {
            const double d = std::abs(pts[i].y() - x);
            if (d < nearestDist) {
                nearestDist = d;
                nearest = i;
            }
        }
        if (nearest >= 0 && nearestDist <= kGrabTolerance) return nearest;
        if (pts.size() >= kMaxPoints) return nearest;

        const double input = inverseRemap(x, pts);
        double lo = 0.0, hi = 1.0;
        for (const QPointF &p : pts) {
            if (p.x() < input) lo = p.x();
            if (p.x() > input && hi == 1.0) hi = p.x();
        }
        lo += kGap;
        hi -= kGap;
        if (lo >= hi) return -1;
        const double clampedIn =
            std::clamp(input, std::max(lo, kClamp), std::min(hi, 1.0 - kClamp));
        int insertAt = pts.size();
        for (int i = 0; i < pts.size(); ++i) {
            if (pts[i].x() > clampedIn) {
                insertAt = i;
                break;
            }
        }
        pts.insert(insertAt, QPointF(clampedIn, x));
        setAnchors(channel, pts);
        return insertAt;
    }

    // Move anchor i's output, clamped between its neighbours' outputs so the
    // map stays monotone and other anchors genuinely never move.
    void moveAnchor(int i, double x) {
        QVector<QPointF> pts = getAnchors(channel);
        if (i < 0 || i >= pts.size()) return;
        const double lo = (i > 0 ? pts[i - 1].y() : 0.0) + kGap;
        const double hi = (i < pts.size() - 1 ? pts[i + 1].y() : 1.0) - kGap;
        const double y = std::clamp(x, std::max(lo, kClamp), std::min(hi, 1.0 - kClamp));
        if (pts[i].y() == y) return;
        pts[i].setY(y);
        setAnchors(channel, pts);
    }

    void drawChannel(QPainter &p, const QRectF &r, int ch, double opacity) {
        quint32 peak = 1;
        for (int i = 0; i < 256; ++i) peak = qMax(peak, bins_[ch * 256 + i]);
        QPainterPath path(QPointF(r.left(), r.bottom()));
        for (int i = 0; i < 256; ++i) {
            const double x = r.left() + r.width() * i / 255.0;
            const double y = r.bottom() - r.height() * double(bins_[ch * 256 + i]) / peak;
            path.lineTo(x, y);
        }
        path.lineTo(r.right(), r.bottom());
        path.closeSubpath();
        QColor fill = kChannelColors[ch];
        fill.setAlphaF(opacity * 0.55);
        QColor stroke = kChannelColors[ch];
        stroke.setAlphaF(opacity);
        p.fillPath(path, fill);
        p.strokePath(path, QPen(stroke, 1));
    }

    QVector<quint32> bins_;
    int dragIndex_ = -1;
};

// ── Window chrome ─────────────────────────────────────────────────────────

LevelsWindow::LevelsWindow(QWidget *parent) : QDialog(parent) {
    setWindowTitle(tr("Interactive Histogram"));
    resize(560, 360);

    auto *layout = new QVBoxLayout(this);
    auto *top = new QHBoxLayout;

    channels_ = new QButtonGroup(this);
    channels_->setExclusive(true);
    for (int i = 0; i < 3; ++i) {
        auto *b = new QPushButton(channelName(i));
        b->setCheckable(true);
        b->setChecked(i == 0);
        channels_->addButton(b, i);
        top->addWidget(b);
    }
    connect(channels_, &QButtonGroup::idClicked, this, &LevelsWindow::channelChanged);

    top->addStretch();
    resetChannel_ = new QPushButton;
    connect(resetChannel_, &QPushButton::clicked, this, &LevelsWindow::resetChannel);
    resetAll_ = new QPushButton(tr("Reset All"));
    connect(resetAll_, &QPushButton::clicked, this, &LevelsWindow::resetAll);
    top->addWidget(resetChannel_);
    top->addWidget(resetAll_);
    layout->addLayout(top);

    plot_ = new LevelsPlot;
    plot_->getAnchors = [this](int ch) { return getAnchors ? getAnchors(ch) : QVector<QPointF>(); };
    plot_->setAnchors = [this](int ch, const QVector<QPointF> &pts) {
        if (setAnchors) setAnchors(ch, pts);
        updateResetButtons();
    };
    plot_->commitHistory = [this](const QString &label) {
        if (commitHistory) commitHistory(label);
    };
    layout->addWidget(plot_, 1);

    auto *hint = new QLabel(
        tr("Drag in the plot to move tones: left of the grab stretches, right "
           "compresses. Release to plant an anchor — further drags work between "
           "anchors; ✕ removes one."));
    hint->setWordWrap(true);
    hint->setStyleSheet("font-size: 11px; color: gray;");
    layout->addWidget(hint);

    channelChanged(0);
}

QString LevelsWindow::channelName(int channel) {
    return channel == 0 ? tr("Red") : channel == 1 ? tr("Green") : tr("Blue");
}

void LevelsWindow::selectChannel(int channel) {
    if (auto *b = channels_->button(channel)) b->setChecked(true);
    channelChanged(channel);
}

void LevelsWindow::setBins(const QVector<quint32> &bins) {
    plot_->setBins(bins);
}

void LevelsWindow::refreshFromSettings() {
    updateResetButtons();
    plot_->update();
}

void LevelsWindow::channelChanged(int channel) {
    channel_ = channel;
    plot_->channel = channel;
    resetChannel_->setText(tr("Reset %1").arg(channelName(channel)));
    updateResetButtons();
    plot_->update();
}

void LevelsWindow::resetChannel() {
    if (setAnchors) setAnchors(channel_, {});
    if (commitHistory) commitHistory(tr("%1 levels reset").arg(channelName(channel_)));
    updateResetButtons();
    plot_->update();
}

void LevelsWindow::resetAll() {
    for (int ch = 0; ch < 3; ++ch)
        if (setAnchors) setAnchors(ch, {});
    if (commitHistory) commitHistory(tr("Levels reset"));
    updateResetButtons();
    plot_->update();
}

void LevelsWindow::updateResetButtons() {
    if (!getAnchors) return;
    bool any = false;
    for (int ch = 0; ch < 3; ++ch)
        if (!getAnchors(ch).isEmpty()) any = true;
    resetChannel_->setEnabled(!getAnchors(channel_).isEmpty());
    resetAll_->setEnabled(any);
}
