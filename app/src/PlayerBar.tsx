import { useEffect, useRef, useState } from "react";

export type PlayerTrack = {
  setId: number;
  title: string;
  subtitle: string;
  src: string;
  peaks: number[];
  duration: number | null;
};

const fmt = (s: number) => {
  if (!isFinite(s)) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
};

export default function PlayerBar({
  track,
  onClose,
}: {
  track: PlayerTrack;
  onClose: () => void;
}) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(track.duration ?? 0);

  // (Re)start on track change.
  useEffect(() => {
    const a = audioRef.current;
    if (!a) return;
    a.src = track.src;
    a.play().catch(() => {});
    setDuration(track.duration ?? 0);
  }, [track]);

  // Progress loop.
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const a = audioRef.current;
      if (a) {
        setTime(a.currentTime);
        if (isFinite(a.duration) && a.duration > 0) setDuration(a.duration);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  // Waveform drawing.
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth;
    const h = cv.clientHeight;
    cv.width = w * dpr;
    cv.height = h * dpr;
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    const peaks = track.peaks.length ? track.peaks : [0.02];
    const progress = duration > 0 ? time / duration : 0;
    const n = peaks.length;
    const barW = w / n;
    for (let i = 0; i < n; i++) {
      const ph = Math.max(peaks[i] * (h - 4), 1.5);
      ctx.fillStyle = i / n <= progress ? "#ffb454" : "#3c3c46";
      ctx.fillRect(i * barW, (h - ph) / 2, Math.max(barW - 0.5, 0.5), ph);
    }
  }, [track.peaks, time, duration]);

  const seek = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const a = audioRef.current;
    const cv = canvasRef.current;
    if (!a || !cv || !duration) return;
    const rect = cv.getBoundingClientRect();
    a.currentTime = ((e.clientX - rect.left) / rect.width) * duration;
  };

  const toggle = () => {
    const a = audioRef.current;
    if (!a) return;
    if (a.paused) a.play().catch(() => {});
    else a.pause();
  };

  return (
    <div className="player">
      <audio
        ref={audioRef}
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
      />
      <button className="play-toggle" onClick={toggle} title={playing ? "Pause" : "Play"}>
        {playing ? "❚❚" : "▶"}
      </button>
      <div className="player-info">
        <div className="player-title">{track.title}</div>
        <div className="player-sub">{track.subtitle}</div>
      </div>
      <canvas ref={canvasRef} className="waveform" onClick={seek} />
      <div className="player-time">
        {fmt(time)} / {fmt(duration)}
      </div>
      <button className="player-close" onClick={onClose} title="Close player">
        ×
      </button>
    </div>
  );
}
