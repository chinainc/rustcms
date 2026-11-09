getRem(375, 100);
window.onresize = function () {
  getRem(375, 100);
};
function getRem(pwidth, prem) {
  var html = document.getElementsByTagName("html")[0];
  var oWidth =
    document.body.clientWidth || document.documentElement.clientWidth;
  if (oWidth < 768) {
    html.style.fontSize = (oWidth / pwidth) * prem + "px";
  } else {
    html.style && html.style.fontSize && (html.style.fontSize = "");
  }
}
$win = $(window);
$(document).on("click", ".m-menu-open", function () {
  $(".header").addClass("m-nav-show");
});
$(document).on("click", ".m-menu-close", function () {
  $(".header").removeClass("m-nav-show");
});

$(document).on("click", ".js-to-top", function () {
  $win.scrollTop(0);
});

function renderTable() {
  var $singleTable = $(".single-arc table");
  if ($singleTable.length > 0) {
    $singleTable.each(function () {
      $(this).wrap('<div class="single-table">');
    });
  }
}

$(document).on("click", "button.accordion-item", function () {
  var contentEl = $(this).next(".accordion-item-content");
  if ($(this).hasClass("active")) {
    $(this).removeClass("active");
    contentEl.css("max-height", 0);
  } else {
    $(this).addClass("active").siblings().removeClass("active");
    contentEl
      .css("max-height", contentEl[0].scrollHeight)
      .siblings(".accordion-item-content")
      .css("max-height", 0);
  }
});

function initial() {
  renderTable();
  $("button.accordion-item").first().click();
}

initial();

$(document).on('click', '.tool-item', function() {
  $(this).addClass('active').siblings().removeClass('active');
})

// 首页 Hero 轮播
$(function () {
  var $slider = $(".fs-hero-slider");
  if (!$slider.length) {
    return;
  }
  var $slides = $slider.find(".fs-hero-slide");
  var $dots = $slider.find(".fs-hero-dots button");
  var index = 0;
  var timer = null;

  function show(i) {
    index = (i + $slides.length) % $slides.length;
    $slides.removeClass("is-active").eq(index).addClass("is-active");
    $dots.removeClass("is-active").eq(index).addClass("is-active");
  }

  function stop() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  function start() {
    stop();
    timer = setInterval(function () {
      show(index + 1);
    }, 5000);
  }

  $slider.find(".fs-hero-arrow--next").on("click", function () {
    show(index + 1);
    start();
  });
  $slider.find(".fs-hero-arrow--prev").on("click", function () {
    show(index - 1);
    start();
  });
  $dots.on("click", function () {
    show($(this).index());
    start();
  });
  $slider.on("mouseenter", stop).on("mouseleave", start);
  show(0);
  start();
});