#include "imageview.h"

#include <QMouseEvent>
#include <QPainter>
#include <QWheelEvent>
#include <algorithm>
#include <cmath>

namespace {
constexpr double kMinBoxSide = 0.02;   // normalized
constexpr double kHandleRadius = 8.0;  // widget px
constexpr double kMaxZoom = 12.0;

// Handle order: TL, TR, BL, BR corners, then T, B, L, R edge midpoints.
QPointF handlePoint(const QRectF &r, int i) {
    switch (i) {
        case 0: return r.topLeft();
        case 1: return r.topRight();
        case 2: return r.bottomLeft();
        case 3: return r.bottomRight();
        case 4: return {r.center().x(), r.top()};
        case 5: return {r.center().x(), r.bottom()};
        case 6: return {r.left(), r.center().y()};
        default: return {r.right(), r.center().y()};
    }
}
}  // namespace

ImageView::ImageView(QWidget *parent) : QWidget(parent) {
    setMinimumSize(400, 300);
    QPalette pal = palette();
    pal.setColor(QPalette::Window, QColor(32, 32, 34));
    setAutoFillBackground(true);
    setPalette(pal);
    setMouseTracking(false);
}

void ImageView::setImage(const QImage &image) {
    const bool sizeChanged = image.size() != image_.size();
    image_ = image;
    if (sizeChanged) clampPan();
    update();
}

void ImageView::setMode(Mode mode) {
    mode_ = mode;
    if (mode_ != Mode::None && zoom_ != 1.0) {  // tools operate on the fitted frame
        zoom_ = 1.0;
        pan_ = QPointF();
        if (onZoomChanged) onZoomChanged(zoom_);
    }
    drag_ = Drag::None;
    update();
}

void ImageView::setBox(const QRectF &normalized) {
    box_ = normalized;
    update();
}

QRectF ImageView::imageDrawRect() const {
    if (image_.isNull()) return {};
    const double fit = std::min(double(width()) / image_.width(),
                                double(height()) / image_.height());
    const double scale = fit * zoom_;
    const QSizeF sz(image_.width() * scale, image_.height() * scale);
    QPointF topLeft((width() - sz.width()) / 2.0, (height() - sz.height()) / 2.0);
    return QRectF(topLeft + pan_, sz);
}

QPointF ImageView::toNormalized(const QPointF &p) const {
    const QRectF r = imageDrawRect();
    if (r.isEmpty()) return {};
    return {(p.x() - r.left()) / r.width(), (p.y() - r.top()) / r.height()};
}

QPointF ImageView::toWidget(const QPointF &n) const {
    const QRectF r = imageDrawRect();
    return {r.left() + n.x() * r.width(), r.top() + n.y() * r.height()};
}

QRectF ImageView::widgetBox() const {
    const QRectF b = box_.isEmpty() ? QRectF(0, 0, 1, 1) : box_;
    return QRectF(toWidget(b.topLeft()), toWidget(b.bottomRight()));
}

int ImageView::hitHandle(const QPointF &p) const {
    const QRectF wb = widgetBox();
    for (int i = 0; i < 8; ++i) {
        if (QLineF(p, handlePoint(wb, i)).length() <= kHandleRadius) return i;
    }
    return -1;
}

void ImageView::clampPan() {
    const QRectF r = imageDrawRect();
    if (r.width() <= width()) {
        pan_.setX(0);
    } else {
        const double maxX = (r.width() - width()) / 2.0;
        pan_.setX(std::clamp(pan_.x(), -maxX, maxX));
    }
    if (r.height() <= height()) {
        pan_.setY(0);
    } else {
        const double maxY = (r.height() - height()) / 2.0;
        pan_.setY(std::clamp(pan_.y(), -maxY, maxY));
    }
}

void ImageView::paintEvent(QPaintEvent *) {
    QPainter p(this);
    if (image_.isNull()) {
        p.setPen(QColor(150, 150, 150));
        p.drawText(rect(), Qt::AlignCenter, tr("Open a folder and pick a frame"));
        return;
    }
    const QRectF dst = imageDrawRect();
    p.setRenderHint(QPainter::SmoothPixmapTransform);
    p.drawImage(dst, image_);

    if (mode_ == Mode::None) return;

    const QRectF wb = widgetBox().intersected(dst);
    // Dim everything outside the box.
    p.setClipRegion(QRegion(dst.toRect()).subtracted(QRegion(wb.toRect())));
    p.fillRect(dst, QColor(0, 0, 0, 140));
    p.setClipping(false);

    if (mode_ == Mode::Crop) {
        p.setRenderHint(QPainter::Antialiasing, false);
        p.setPen(QPen(QColor(255, 255, 255, 90), 1));
        for (int t = 1; t < 3; ++t) {  // thirds
            const double x = wb.left() + wb.width() * t / 3.0;
            const double y = wb.top() + wb.height() * t / 3.0;
            p.drawLine(QPointF(x, wb.top()), QPointF(x, wb.bottom()));
            p.drawLine(QPointF(wb.left(), y), QPointF(wb.right(), y));
        }
        p.setPen(QPen(Qt::white, 1.5));
        p.setBrush(Qt::NoBrush);
        p.drawRect(wb);
        p.setBrush(Qt::white);
        for (int i = 0; i < 8; ++i) {
            const QPointF c = handlePoint(wb, i);
            p.drawRect(QRectF(c.x() - 3, c.y() - 3, 6, 6));
        }
    } else {  // AnalysisDraw
        p.setPen(QPen(QColor(255, 170, 40), 1.5, Qt::DashLine));
        p.setBrush(Qt::NoBrush);
        p.drawRect(wb);
    }
}

void ImageView::mousePressEvent(QMouseEvent *event) {
    if (event->button() != Qt::LeftButton || image_.isNull()) return;
    const QPointF pos = event->position();
    dragStartWidget_ = pos;

    if (mode_ == Mode::None) {
        if (zoom_ > 1.0) {
            drag_ = Drag::Pan;
            panStart_ = pan_;
            setCursor(Qt::ClosedHandCursor);
        }
        return;
    }
    if (mode_ == Mode::Crop) {
        dragHandle_ = hitHandle(pos);
        dragStartBox_ = box_.isEmpty() ? QRectF(0, 0, 1, 1) : box_;
        if (dragHandle_ >= 0) {
            drag_ = Drag::Handle;
        } else if (widgetBox().contains(pos)) {
            drag_ = Drag::Move;
        } else {
            drag_ = Drag::NewRect;
        }
        return;
    }
    // AnalysisDraw: every press starts a fresh rect.
    drag_ = Drag::NewRect;
}

void ImageView::mouseMoveEvent(QMouseEvent *event) {
    if (drag_ == Drag::None) return;
    const QPointF pos = event->position();

    if (drag_ == Drag::Pan) {
        pan_ = panStart_ + (pos - dragStartWidget_);
        clampPan();
        update();
        return;
    }

    const QPointF n0 = toNormalized(dragStartWidget_);
    const QPointF n1 = toNormalized(pos);

    if (drag_ == Drag::NewRect) {
        QRectF b(QPointF(std::min(n0.x(), n1.x()), std::min(n0.y(), n1.y())),
                 QPointF(std::max(n0.x(), n1.x()), std::max(n0.y(), n1.y())));
        box_ = b.intersected(QRectF(0, 0, 1, 1));
    } else if (drag_ == Drag::Move) {
        QPointF d = n1 - n0;
        d.setX(std::clamp(d.x(), -dragStartBox_.left(), 1.0 - dragStartBox_.right()));
        d.setY(std::clamp(d.y(), -dragStartBox_.top(), 1.0 - dragStartBox_.bottom()));
        box_ = dragStartBox_.translated(d);
    } else {  // Handle
        QRectF b = dragStartBox_;
        const QPointF d = n1 - n0;
        const bool left = dragHandle_ == 0 || dragHandle_ == 2 || dragHandle_ == 6;
        const bool right = dragHandle_ == 1 || dragHandle_ == 3 || dragHandle_ == 7;
        const bool top = dragHandle_ == 0 || dragHandle_ == 1 || dragHandle_ == 4;
        const bool bottom = dragHandle_ == 2 || dragHandle_ == 3 || dragHandle_ == 5;
        if (left) b.setLeft(std::clamp(b.left() + d.x(), 0.0, b.right() - kMinBoxSide));
        if (right) b.setRight(std::clamp(b.right() + d.x(), b.left() + kMinBoxSide, 1.0));
        if (top) b.setTop(std::clamp(b.top() + d.y(), 0.0, b.bottom() - kMinBoxSide));
        if (bottom) b.setBottom(std::clamp(b.bottom() + d.y(), b.top() + kMinBoxSide, 1.0));
        box_ = b;
    }
    update();
}

void ImageView::mouseReleaseEvent(QMouseEvent *event) {
    if (event->button() != Qt::LeftButton) return;
    const Drag was = drag_;
    drag_ = Drag::None;
    if (was == Drag::Pan) {
        setCursor(Qt::ArrowCursor);
        return;
    }
    if (was != Drag::None && mode_ != Mode::None) {
        // Degenerate new-rects clear the box (a stray click isn't a crop).
        if (was == Drag::NewRect &&
            (box_.width() < kMinBoxSide || box_.height() < kMinBoxSide)) {
            box_ = QRectF();
            update();
        }
        if (onBoxCommitted) onBoxCommitted();
    }
}

void ImageView::mouseDoubleClickEvent(QMouseEvent *event) {
    if (mode_ != Mode::None || image_.isNull()) return;
    // Toggle fit ↔ 100% around the cursor.
    if (zoom_ > 1.0) {
        zoom_ = 1.0;
        pan_ = QPointF();
        if (onZoomChanged) onZoomChanged(zoom_);
    } else {
        const double fit = std::min(double(width()) / image_.width(),
                                    double(height()) / image_.height());
        const QPointF anchor = toNormalized(event->position());
        zoom_ = std::min(1.0 / fit, kMaxZoom);  // 1 image px = 1 widget px
        const QRectF r = imageDrawRect();
        const QPointF target(anchor.x() * r.width(), anchor.y() * r.height());
        pan_ = QPointF(width() / 2.0, height() / 2.0) - target -
               (QPointF((width() - r.width()) / 2.0, (height() - r.height()) / 2.0));
        clampPan();
        if (onZoomChanged) onZoomChanged(zoom_);
    }
    update();
}

void ImageView::wheelEvent(QWheelEvent *event) {
    if (mode_ != Mode::None || image_.isNull()) return;
    const double factor = std::pow(1.0015, event->angleDelta().y());
    const double newZoom = std::clamp(zoom_ * factor, 1.0, kMaxZoom);
    if (newZoom == zoom_) return;
    // Keep the image point under the cursor stationary.
    const QPointF anchor = toNormalized(event->position());
    zoom_ = newZoom;
    if (zoom_ == 1.0) {
        pan_ = QPointF();
    } else {
        const QRectF r = imageDrawRect();
        const QPointF anchorWidget(r.left() + anchor.x() * r.width(),
                                   r.top() + anchor.y() * r.height());
        pan_ += event->position() - anchorWidget;
        clampPan();
    }
    if (onZoomChanged) onZoomChanged(zoom_);
    update();
}
