// Canvas: fit-to-window image with wheel zoom / drag pan, plus two overlay
// tool modes — an axis-aligned crop box with handles and a draw-a-rect
// analysis region. Coordinates crossing the boundary are NORMALIZED to the
// currently displayed bitmap (which is what the settings store), so the
// window layer never sees pixels.
#ifndef SWIFTINVERT_IMAGEVIEW_H
#define SWIFTINVERT_IMAGEVIEW_H

#include <QWidget>
#include <functional>

class ImageView : public QWidget {
public:
    enum class Mode { None, Crop, AnalysisDraw };

    explicit ImageView(QWidget *parent = nullptr);

    void setImage(const QImage &image);
    void setCanvasColor(const QColor &color);
    void setMode(Mode mode);
    Mode mode() const { return mode_; }

    // The active tool rect, normalized to the displayed image. In Crop mode
    // an empty box means "full frame".
    void setBox(const QRectF &normalized);
    QRectF box() const { return box_; }

    // Fired after any user edit of the box (drag/resize/new rect), on
    // mouse release.
    std::function<void()> onBoxCommitted;

    // Fired whenever the zoom factor changes (1.0 = fit).
    std::function<void(double)> onZoomChanged;
    double zoomFactor() const { return zoom_; }

protected:
    void paintEvent(QPaintEvent *) override;
    void mousePressEvent(QMouseEvent *) override;
    void mouseMoveEvent(QMouseEvent *) override;
    void mouseReleaseEvent(QMouseEvent *) override;
    void mouseDoubleClickEvent(QMouseEvent *) override;
    void wheelEvent(QWheelEvent *) override;

private:
    enum class Drag { None, Pan, Move, NewRect, Handle };

    QRectF imageDrawRect() const;  // widget-space rect the bitmap occupies
    QPointF toNormalized(const QPointF &widgetPoint) const;
    QPointF toWidget(const QPointF &normalizedPoint) const;
    QRectF widgetBox() const;
    int hitHandle(const QPointF &widgetPoint) const;  // -1 = none
    void clampPan();

    QImage image_;
    Mode mode_ = Mode::None;
    QRectF box_;  // normalized; empty = unset/full
    double zoom_ = 1.0;
    QPointF pan_;  // widget-space offset from centered position

    Drag drag_ = Drag::None;
    int dragHandle_ = -1;
    QPointF dragStartWidget_;
    QRectF dragStartBox_;
    QPointF panStart_;
};

#endif
