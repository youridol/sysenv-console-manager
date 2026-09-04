import * as React from "react";
import useEmblaCarousel from "embla-carousel-react";
import { cn } from "../../lib/utils";
import { Button } from "./button";
import { ChevronLeft, ChevronRight } from "lucide-react";

type CarouselApi = ReturnType<typeof useEmblaCarousel>[1];

interface CarouselContextValue {
  carouselRef: ReturnType<typeof useEmblaCarousel>[0];
  api: CarouselApi | undefined;
  canScrollPrev: boolean;
  canScrollNext: boolean;
  scrollPrev: () => void;
  scrollNext: () => void;
}

const CarouselContext = React.createContext<CarouselContextValue | null>(null);

function useCarousel() {
  const ctx = React.useContext(CarouselContext);
  if (!ctx) throw new Error("useCarousel must be used within <Carousel>");
  return ctx;
}

function Carousel({
  className,
  children,
  ...props
}: React.ComponentProps<"div">) {
  const [carouselRef, api] = useEmblaCarousel({ loop: false, align: "start" });
  const [canScrollPrev, setCanScrollPrev] = React.useState(false);
  const [canScrollNext, setCanScrollNext] = React.useState(false);

  const onSelect = React.useCallback((e: any) => {
    if (!e) return;
    setCanScrollPrev(e.canScrollPrev());
    setCanScrollNext(e.canScrollNext());
  }, []);

  React.useEffect(() => {
    if (!api) return;
    onSelect(api);
    api.on("select", onSelect);
    api.on("reInit", onSelect);
    return () => {
      api.off("select", onSelect);
      api.off("reInit", onSelect);
    };
  }, [api, onSelect]);

  return (
    <CarouselContext.Provider
      value={{
        carouselRef,
        api: api ?? undefined,
        canScrollPrev,
        canScrollNext,
        scrollPrev: () => api?.scrollPrev(),
        scrollNext: () => api?.scrollNext(),
      }}
    >
      <div
        data-slot="carousel"
        className={cn("relative", className)}
        {...props}
      >
        {children}
      </div>
    </CarouselContext.Provider>
  );
}

function CarouselContent({ className, ...props }: React.ComponentProps<"div">) {
  const { carouselRef } = useCarousel();
  return (
    <div ref={carouselRef} className="overflow-hidden">
      {/* 间距用 item 内 padding 实现（调用方以 -ml-4 补偿首项），
          不用 flex gap：gap 在 basis 百分比之外累加会导致内容溢出容器
          （v0.20.0 溢出事故根因：basis-1/2 + gap-4 = 100% + 16px） */}
      <div
        data-slot="carousel-content"
        className={cn("flex", className)}
        {...props}
      />
    </div>
  );
}

function CarouselItem({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="carousel-item"
      className={cn(
        "min-w-0 shrink-0 grow-0 basis-full pl-4",
        className,
      )}
      {...props}
    />
  );
}

function CarouselPrevious() {
  const { canScrollPrev, scrollPrev } = useCarousel();
  return (
    <Button
      variant="outline"
      size="icon"
      className="absolute -left-3 top-1/2 -translate-y-1/2 z-10 h-8 w-8 rounded-full shadow"
      disabled={!canScrollPrev}
      onClick={scrollPrev}
    >
      <ChevronLeft className="h-4 w-4" />
    </Button>
  );
}

function CarouselNext() {
  const { canScrollNext, scrollNext } = useCarousel();
  return (
    <Button
      variant="outline"
      size="icon"
      className="absolute -right-3 top-1/2 -translate-y-1/2 z-10 h-8 w-8 rounded-full shadow"
      disabled={!canScrollNext}
      onClick={scrollNext}
    >
      <ChevronRight className="h-4 w-4" />
    </Button>
  );
}

export {
  Carousel,
  CarouselContent,
  CarouselItem,
  CarouselPrevious,
  CarouselNext,
};
