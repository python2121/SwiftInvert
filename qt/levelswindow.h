// The Interactive Histogram window (double-click the sidebar histogram) —
// the Qt port of the Mac's InteractiveHistogram: a large R/G/B histogram
// where each channel's tonal axis is reshaped by dragging. A drag grabs a
// tone (or an existing anchor) and moves its output; releasing PLANTS an
// anchor that stays fixed — later drags reshape only their own segment
// between neighbouring anchors. Anchors show as vertical lines with an ✕
// below to remove. The remap applies inside the render kernels, so the
// plot re-bins live under the drag.
//
// Constants and semantics mirror the Mac exactly: gap 0.01, grab tolerance
// 0.015, endpoint clamp 0.02 (ExposureSettings.levelsClamp), max 8 anchors,
// and the inverse remap is ReferenceCurve.levelsInverseRemap re-expressed
// here (the kernel remains the rendering source of truth).
#ifndef SWIFTINVERT_LEVELSWINDOW_H
#define SWIFTINVERT_LEVELSWINDOW_H

#include <QDialog>
#include <QPointF>
#include <QVector>
#include <functional>

class QButtonGroup;
class QPushButton;

class LevelsPlot;

class LevelsWindow : public QDialog {
public:
    explicit LevelsWindow(QWidget *parent = nullptr);

    // Host callbacks: read/write one channel's anchors ([input, output]
    // pairs, sorted by input) and commit a history entry.
    std::function<QVector<QPointF>(int channel)> getAnchors;
    std::function<void(int channel, const QVector<QPointF> &)> setAnchors;
    std::function<void(const QString &label)> commitHistory;

    void setBins(const QVector<quint32> &bins);  // 4×256, live from renders
    void refreshFromSettings();                  // anchors changed elsewhere
    void selectChannel(int channel);

private:
    void channelChanged(int channel);
    void resetChannel();
    void resetAll();
    void updateResetButtons();
    static QString channelName(int channel);

    LevelsPlot *plot_ = nullptr;
    QButtonGroup *channels_ = nullptr;
    QPushButton *resetChannel_ = nullptr;
    QPushButton *resetAll_ = nullptr;
    int channel_ = 0;
};

#endif
